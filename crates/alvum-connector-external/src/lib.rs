//! External HTTP extension runtime.
//!
//! This crate adapts manifest-defined extension packages into Alvum's internal
//! Connector/CaptureSource/Processor interfaces.

pub mod analysis;
pub mod client;
pub mod connector;
pub mod registry;

pub use analysis::{AnalysisArtifact, AnalysisResponse, run_analysis, run_enabled_analyses};
pub use client::{
    CaptureStartRequest, CaptureStartResponse, CaptureStopRequest, ExtensionClient, GatherRequest,
    GatherResponse, ManagedExtension, ProcessRequest, ProcessResponse,
};
pub use connector::{
    ExternalCaptureSource, ExternalConnector, ExternalProcessor, capture_sources_from_config,
    connectors_from_config,
};
pub use registry::{ExtensionInstallSource, ExtensionRegistryStore};
