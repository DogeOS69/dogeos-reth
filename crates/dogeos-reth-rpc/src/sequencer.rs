use alloy_json_rpc::{RpcRecv, RpcSend};
use alloy_primitives::{B256, hex};
use alloy_rpc_client::{BuiltInConnectionString, ClientBuilder, RpcClient};
use alloy_transport::{RpcError, TransportError, TransportErrorKind};
use alloy_transport_http::Http;
use std::{str::FromStr, sync::Arc};
use thiserror::Error;
use tracing::warn;

/// Failure while creating a sequencer RPC client.
#[derive(Debug, Error)]
pub enum SequencerConnectError {
    #[error("invalid sequencer URL: {0}")]
    InvalidUrl(String),
    #[error("failed to connect to sequencer: {0}")]
    Transport(#[from] TransportError),
    #[error("failed to initialize sequencer HTTP client: {0}")]
    HttpClient(#[from] reqwest::Error),
}

/// Failure returned by a sequencer JSON-RPC request.
#[derive(Debug, Error)]
pub enum SequencerClientError {
    #[error(transparent)]
    Rpc(#[from] RpcError<TransportErrorKind>),
}

/// Client used to forward raw transactions to an external sequencer.
#[derive(Debug, Clone)]
pub struct SequencerClient {
    inner: Arc<SequencerClientInner>,
}

impl SequencerClient {
    /// Creates a client for HTTP(S) or WebSocket sequencer endpoints.
    pub async fn new(endpoint: impl Into<String>) -> Result<Self, SequencerConnectError> {
        let endpoint = endpoint.into();
        let connection = BuiltInConnectionString::from_str(&endpoint)?;
        if let BuiltInConnectionString::Http(url) = connection {
            let client = reqwest::Client::builder().use_rustls_tls().build()?;
            Self::with_http_client(url, client)
        } else {
            let client = ClientBuilder::default().connect_with(connection).await?;
            Ok(Self::from_client(endpoint, client))
        }
    }

    /// Creates an HTTP sequencer client using a caller-supplied reqwest client.
    pub fn with_http_client(
        endpoint: impl Into<String>,
        client: reqwest::Client,
    ) -> Result<Self, SequencerConnectError> {
        let endpoint = endpoint.into();
        let url = endpoint
            .parse()
            .map_err(|_| SequencerConnectError::InvalidUrl(endpoint.clone()))?;
        let transport = Http::with_client(client, url);
        let is_local = transport.guess_local();
        let client = ClientBuilder::default().transport(transport, is_local);
        Ok(Self::from_client(endpoint, client))
    }

    fn from_client(endpoint: String, client: RpcClient) -> Self {
        Self {
            inner: Arc::new(SequencerClientInner { endpoint, client }),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.inner.endpoint
    }

    pub fn client(&self) -> &RpcClient {
        &self.inner.client
    }

    async fn send_rpc_call<Params: RpcSend, Resp: RpcRecv>(
        &self,
        method: &str,
        params: Params,
    ) -> Result<Resp, SequencerClientError> {
        self.client()
            .request(method.to_owned(), params)
            .await
            .inspect_err(|error| {
                warn!(target: "dogeos::rpc::sequencer", %error, method, "sequencer RPC request failed");
            })
            .map_err(Into::into)
    }

    /// Forwards an encoded transaction through `eth_sendRawTransaction`.
    pub async fn forward_raw_transaction(
        &self,
        transaction: &[u8],
    ) -> Result<B256, SequencerClientError> {
        let encoded = hex::encode_prefixed(transaction);
        self.send_rpc_call("eth_sendRawTransaction", (encoded,))
            .await
    }
}

#[derive(Debug)]
struct SequencerClientInner {
    endpoint: String,
    client: RpcClient,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_client_preserves_endpoint_and_rpc_encoding() {
        let client =
            SequencerClient::with_http_client("http://localhost:8545", reqwest::Client::new())
                .unwrap();
        assert_eq!(client.endpoint(), "http://localhost:8545");

        let request = client
            .client()
            .make_request("eth_sendRawTransaction", (hex::encode_prefixed(b"abcd"),))
            .serialize()
            .unwrap()
            .take_request();
        assert_eq!(
            request.get(),
            r#"{"method":"eth_sendRawTransaction","params":["0x61626364"],"id":0,"jsonrpc":"2.0"}"#
        );
    }

    #[test]
    fn malformed_http_endpoint_is_rejected() {
        let error =
            SequencerClient::with_http_client("not a URL", reqwest::Client::new()).unwrap_err();
        assert!(matches!(error, SequencerConnectError::InvalidUrl(_)));
    }
}
