//! Timestamp query sets: a counter sample buffer that passes sample into at their boundaries
//! and `resolve_query_set` resolves into a buffer, as wgpu-hal does.

use std::sync::Arc;

use objc2_foundation_video::NSString;
use objc2_metal::{
    MTLCounterDontSample, MTLCounterSampleBuffer, MTLCounterSampleBufferDescriptor, MTLCounterSamplingPoint, MTLCounterSet, MTLDevice,
    MTLStorageMode,
};
use wgpu::custom::*;

use super::{Obj, send_sync};

#[derive(Debug)]
pub(crate) struct QuerySet {
    pub(super) raw: Arc<QueryRaw>,
}

#[derive(Debug)]
pub(crate) struct QueryRaw(pub(super) Obj<dyn MTLCounterSampleBuffer>);
send_sync!(QueryRaw);

/// Whether passes can sample timestamps at their boundaries.
pub(super) fn supported(device: &Obj<dyn MTLDevice>) -> bool { device.supportsCounterSampling(MTLCounterSamplingPoint::AtStageBoundary) }

impl QuerySet {
    pub(super) fn new(device: &Obj<dyn MTLDevice>, desc: &wgpu::QuerySetDescriptor<'_>) -> Self {
        if !matches!(desc.ty, wgpu::QueryType::Timestamp) { super::unsupported!("{:?} queries", desc.ty) }
        let d = MTLCounterSampleBufferDescriptor::new();
        d.setStorageMode(MTLStorageMode::Shared);
        // SAFETY: wgpu limits a query set to 4096 queries, within Metal's sample buffer size.
        unsafe { d.setSampleCount(desc.count.max(1) as usize) };
        if let Some(label) = desc.label { d.setLabel(&NSString::from_str(label)); }
        let sets = device.counterSets().expect("Metal counter sets");
        let timestamp = sets.iter().find(|s| s.name().to_string() == "timestamp").expect("a Metal timestamp counter set");
        d.setCounterSet(Some(&timestamp));
        let raw = device.newCounterSampleBufferWithDescriptor_error(&d).unwrap_or_else(|e| panic!("a Metal counter sample buffer: {}", e.localizedDescription()));
        QuerySet { raw: Arc::new(QueryRaw(raw)) }
    }
}

impl QuerySetInterface for QuerySet {
    fn destroy(&self) {}
}

/// A pass's timestamp indices as Metal takes them.
pub(super) fn index(at: Option<u32>) -> usize { at.map_or(MTLCounterDontSample, |i| i as usize) }
