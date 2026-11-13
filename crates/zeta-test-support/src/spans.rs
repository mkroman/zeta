//! A `tracing` layer recording span fields for assertions.
//!
//! Tests that assert on exported span attributes install this layer with a fresh registry and
//! read the recorded spans back. It lives here rather than in the consuming test modules because
//! both the dispatcher's span tests and the host's telemetry tests need the same recorder.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// One span as the recorder observed it.
#[derive(Clone, Debug)]
pub struct RecordedSpan {
    /// The span's identity, used to merge fields recorded after creation.
    id: Id,
    /// The span's static name.
    name: String,
    /// The span that was current when this one was created; a root has no parent.
    parent: Option<Id>,
    /// Every field recorded on the span, at creation or later.
    fields: BTreeMap<String, String>,
}

impl RecordedSpan {
    /// Returns the span's static name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the span's identity, as assigned by the subscriber.
    #[must_use]
    pub const fn id(&self) -> &Id {
        &self.id
    }

    /// Returns the id of the span that was current when this one was created.
    #[must_use]
    pub const fn parent(&self) -> Option<&Id> {
        self.parent.as_ref()
    }

    /// Returns the string value of `field`.
    #[must_use]
    pub fn field(&self, field: &str) -> Option<&str> {
        self.fields.get(field).map(String::as_str)
    }
}

/// Records every span created while it is installed.
#[derive(Clone, Default)]
pub struct SpanRecorder(Arc<Mutex<Vec<RecordedSpan>>>);

impl SpanRecorder {
    /// Returns a copy of the spans recorded so far.
    ///
    /// # Panics
    ///
    /// Panics if the recorder's lock is poisoned, which means a test panicked while holding it.
    #[must_use]
    pub fn spans(&self) -> Vec<RecordedSpan> {
        self.0
            .lock()
            .expect("span recorder should not be poisoned")
            .clone()
    }
}

/// Collects the fields of a span, rendered as their exported values.
#[derive(Default)]
struct Fields(BTreeMap<String, String>);

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

impl<S> Layer<S> for SpanRecorder
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let parent = if attrs.is_contextual() {
            ctx.lookup_current().map(|span| span.id())
        } else {
            None
        };

        let mut fields = Fields::default();
        attrs.record(&mut fields);

        let mut spans = self.0.lock().expect("span recorder should not be poisoned");
        spans.push(RecordedSpan {
            id: id.clone(),
            name: attrs.metadata().name().to_owned(),
            parent,
            fields: fields.0,
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        values.record(&mut fields);

        self.0
            .lock()
            .expect("span recorder should not be poisoned")
            .iter_mut()
            .find(|span| span.id == *id)
            .expect("recorded spans should exist")
            .fields
            .extend(fields.0);
    }
}
