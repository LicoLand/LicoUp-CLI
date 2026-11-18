//! Segments, phases, and span links (contract C07).
//!
//! Waiting and work are recorded as separate segments so a slow turn can be
//! attributed instead of merely observed. Parallel predecessors and
//! asynchronous queues are expressed with [`ObservationSpanLink`] values, so no
//! reader has to infer a synchronous call stack that never existed.

use serde::{Deserialize, Serialize};

use super::correlation::CorrelationIds;

/// The measured interval a segment describes.
///
/// The set is the contract's telemetry list: admission wait, DB transaction,
/// queue wait, adapter first event, prepare CPU, prepare queue, build, raster,
/// and input display.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ObservationPhase {
    /// Waiting before an effect is admitted.
    AdmissionWait,
    /// A database transaction.
    DatabaseTransaction,
    /// Waiting in an asynchronous queue.
    QueueWait,
    /// Latency from dispatch until the adapter's first event.
    AdapterFirstEvent,
    /// CPU spent preparing an input.
    PrepareCpu,
    /// Waiting for prepare capacity.
    PrepareQueue,
    /// Rendering build work.
    Build,
    /// Raster work.
    Raster,
    /// User-visible input-to-display latency.
    InputDisplay,
}

impl ObservationPhase {
    /// Every phase, in contract order.
    pub const ALL: [ObservationPhase; 9] = [
        ObservationPhase::AdmissionWait,
        ObservationPhase::DatabaseTransaction,
        ObservationPhase::QueueWait,
        ObservationPhase::AdapterFirstEvent,
        ObservationPhase::PrepareCpu,
        ObservationPhase::PrepareQueue,
        ObservationPhase::Build,
        ObservationPhase::Raster,
        ObservationPhase::InputDisplay,
    ];

    /// The stable wire name of this phase.
    pub const fn wire(self) -> &'static str {
        match self {
            Self::AdmissionWait => "admission_wait",
            Self::DatabaseTransaction => "database_transaction",
            Self::QueueWait => "queue_wait",
            Self::AdapterFirstEvent => "adapter_first_event",
            Self::PrepareCpu => "prepare_cpu",
            Self::PrepareQueue => "prepare_queue",
            Self::Build => "build",
            Self::Raster => "raster",
            Self::InputDisplay => "input_display",
        }
    }

    /// Whether the interval is time spent waiting or time spent working.
    ///
    /// `InputDisplay` is waiting: it is the latency a person perceived, while
    /// any work inside it is already attributed to a build or raster segment.
    pub const fn kind(self) -> ObservationSegmentKind {
        match self {
            Self::AdmissionWait
            | Self::QueueWait
            | Self::AdapterFirstEvent
            | Self::PrepareQueue
            | Self::InputDisplay => ObservationSegmentKind::Wait,
            Self::DatabaseTransaction | Self::PrepareCpu | Self::Build | Self::Raster => {
                ObservationSegmentKind::Work
            }
        }
    }
}

/// Waiting versus work.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSegmentKind {
    /// Time a caller spent waiting; no progress happened here.
    Wait,
    /// Time spent making progress.
    Work,
}

impl ObservationSegmentKind {
    /// The stable wire name of this kind.
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Wait => "wait",
            Self::Work => "work",
        }
    }
}

/// Why two segments are related without one calling the other.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ObservationLinkRelation {
    /// The linked segment is one of several parallel predecessors of this one.
    Predecessor,
    /// The linked segment produced the queue item this one consumed.
    QueueProducer,
}

impl ObservationLinkRelation {
    /// The stable wire name of this relation.
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Predecessor => "predecessor",
            Self::QueueProducer => "queue_producer",
        }
    }
}

/// A relation from one segment to another causal context.
///
/// Links are the contract's answer to parallel predecessors and asynchronous
/// queues: the linked context owns its own timing, and this segment only states
/// how it depends on it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationSpanLink {
    /// Correlation ids of the linked context.
    pub correlation: CorrelationIds,
    /// How the linked context relates to this segment.
    pub relation: ObservationLinkRelation,
}

impl ObservationSpanLink {
    /// Builds a link.
    pub fn new(correlation: CorrelationIds, relation: ObservationLinkRelation) -> Self {
        Self {
            correlation,
            relation,
        }
    }
}

/// One completed waiting or work interval.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationSegmentRecord {
    /// Which interval was measured.
    pub phase: ObservationPhase,
    /// Ids of this segment's causal context.
    pub correlation: CorrelationIds,
    /// Clock reading when the segment opened, in microseconds.
    pub started_micros: u64,
    /// How long the segment stayed open, in microseconds.
    pub duration_micros: u64,
    /// Parallel predecessors and queue producers, bounded by the privacy budget.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ObservationSpanLink>,
}

impl ObservationSegmentRecord {
    /// Builds a segment record.
    pub fn new(
        phase: ObservationPhase,
        correlation: CorrelationIds,
        started_micros: u64,
        duration_micros: u64,
    ) -> Self {
        Self {
            phase,
            correlation,
            started_micros,
            duration_micros,
            links: Vec::new(),
        }
    }

    /// Whether this record describes waiting or work.
    pub const fn kind(&self) -> ObservationSegmentKind {
        self.phase.kind()
    }

    /// Attaches a link to a parallel predecessor or queue producer.
    pub fn with_link(
        mut self,
        correlation: CorrelationIds,
        relation: ObservationLinkRelation,
    ) -> Self {
        self.links
            .push(ObservationSpanLink::new(correlation, relation));
        self
    }
}

/// An open segment. Holding one costs a clock reading and nothing else; the
/// probe it came from reads no clock while switched off.
#[derive(Clone)]
pub struct ActiveObservationSegment {
    phase: ObservationPhase,
    correlation: CorrelationIds,
    started_micros: u64,
    links: Vec<ObservationSpanLink>,
    clock: super::probe::ObservationClock,
}

impl std::fmt::Debug for ActiveObservationSegment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveObservationSegment")
            .field("phase", &self.phase)
            .field("kind", &self.phase.kind())
            .field("correlation", &self.correlation)
            .field("started_micros", &self.started_micros)
            .field("links", &self.links.len())
            .finish()
    }
}

impl ActiveObservationSegment {
    pub(super) fn open(
        phase: ObservationPhase,
        correlation: CorrelationIds,
        started_micros: u64,
        clock: super::probe::ObservationClock,
    ) -> Self {
        Self {
            phase,
            correlation,
            started_micros,
            links: Vec::new(),
            clock,
        }
    }

    /// Which interval this segment measures.
    pub const fn phase(&self) -> ObservationPhase {
        self.phase
    }

    /// Ids carried by this segment.
    pub const fn correlation(&self) -> &CorrelationIds {
        &self.correlation
    }

    /// Attaches a link to a parallel predecessor or queue producer.
    pub fn add_link(&mut self, correlation: CorrelationIds, relation: ObservationLinkRelation) {
        self.links
            .push(ObservationSpanLink::new(correlation, relation));
    }

    /// Closes the segment and reads the clock once more.
    pub fn finish(self) -> ObservationSegmentRecord {
        let finished_micros = (self.clock)();
        ObservationSegmentRecord {
            phase: self.phase,
            correlation: self.correlation,
            started_micros: self.started_micros,
            duration_micros: finished_micros.saturating_sub(self.started_micros),
            links: self.links,
        }
    }
}
