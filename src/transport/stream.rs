// FILE: src/transport/stream.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Define a transport-agnostic stream contract and resolved stream metadata shared by adapters.
//   SCOPE: Async read or write split types, transport kind tagging, shutdown errors, and generic stream behavior.
//   DEPENDS: std, async-trait, tokio, thiserror
//   LINKS: M-WSS-GATEWAY, M-IROH-ADAPTER, M-PROXY-BRIDGE
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   BoxedRead - boxed async read half used by ProxyBridge
//   BoxedWrite - boxed async write half used by ProxyBridge
//   TransportStream - minimal stream trait for adapter outputs
//   ResolvedStream - transport stream plus transport kind metadata
//   TransportKind - Wss, IrohDirect, or IrohRelay discriminator for metrics
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added the missing change summary block to keep the shared transport stream contract release-ready under GRACE review.
// END_CHANGE_SUMMARY

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

pub type BoxedRead = Pin<Box<dyn AsyncRead + Send>>;
pub type BoxedWrite = Pin<Box<dyn AsyncWrite + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Wss,
    IrohDirect,
    IrohRelay,
}

pub struct ResolvedStream {
    pub stream: Box<dyn TransportStream>,
    pub transport_kind: TransportKind,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ShutdownError {
    #[error("stream shutdown timed out")]
    Timeout,
}

#[async_trait]
pub trait TransportStream: Send + 'static {
    fn split(self: Box<Self>) -> (BoxedRead, BoxedWrite);
    fn peer_label(&self) -> &str;
    async fn shutdown(self: Box<Self>, timeout: Duration) -> Result<(), ShutdownError>;
}
