//! `tracing` helpers: a layer recording span fields, and a writer capturing log output.
//!
//! Tests that assert on exported span attributes install the layer with a fresh registry and
//! read the recorded spans back. Tests that assert on log lines install a formatting subscriber
//! whose writer is a [`LogBuffer`] and read the captured bytes back. Both live here rather than
//! in the consuming test modules because several crates need the same helpers.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// A writer that collects everything logged through it, for assertions.
///
/// Every clone shares the same buffer, so a subscriber can own one handle while the test keeps
/// another. It is the writer half of a capturing `tracing_subscriber` formatting layer.
#[derive(Clone, Debug, Default)]
pub struct LogBuffer(
    /// The bytes written through the buffer so far.
    pub Arc<Mutex<Vec<u8>>>,
);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer lock")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

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
#[derive(Clone, Debug, Default)]
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

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use tracing_subscriber::fmt::MakeWriter as _;

    use super::LogBuffer;

    /// A writer taken from a clone appends to the buffer the other handles see.
    #[test]
    fn log_buffer_collects_writes_from_every_handle() {
        let buffer = LogBuffer::default();
        let mut writer = buffer.make_writer();

        writer
            .write_all(b"hello ")
            .expect("the buffer write succeeds");
        writer
            .write_all(b"world")
            .expect("the buffer write succeeds");
        writer.flush().expect("the buffer flush succeeds");

        let logged = buffer.0.lock().expect("log buffer lock");
        assert_eq!(&logged[..], b"hello world");
    }
}
