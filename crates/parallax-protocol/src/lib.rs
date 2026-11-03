//! The parallax wire protocol and layout documents.
//!
//! [`v1`] holds the protobuf messages generated from `proto/parallax.proto`.
//! [`layout`] defines the layout JSON document that describes a view, and turns it
//! into an engine configuration against a table's schema. [`ipc`] encodes record
//! batches as Arrow IPC streams for frames.

pub mod ipc;
pub mod layout;

/// Messages generated from `proto/parallax.proto`.
#[allow(missing_docs, clippy::all, clippy::pedantic)]
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/parallax.v1.rs"));
}

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;
