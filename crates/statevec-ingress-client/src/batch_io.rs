// Included in the single client port module. Only partial I/O and readiness
// live here; all connection/retry/deadline/terminal decisions stay in the owner.
use ingress_api::stream_batch::{FrameReader, ReadFailure, ReadProgress};
use std::num::NonZeroU64;
use std::task::Poll;

/// Mechanical custody of one already connected, nonblocking byte stream.
///
/// Construct through `IngressClientOwner::bind_batch_stream` after forwarding
/// the actual Connected observation. `S` must perform nonblocking Read/Write
/// (a virtual byte device is also valid). Connection establishment and readiness
/// registration are supplied by the caller; this port never connects or retries.
/// A poll performs at most one syscall and alternates runnable read/write lanes.
pub struct BatchIoWorker<S: Read + Write> {
    stream: S,
    operation_id: u64,
    reader: Option<FrameReader>,
    write: Option<(NonZeroU64, Vec<u8>, usize)>,
    // Mechanical readiness hints, not semantic permission. Only WouldBlock
    // clears them; positive progress must not require another readiness edge.
    readable: bool,
    writable: bool,
    write_first: bool,
}

impl<S: Read + Write> BatchIoWorker<S> {
    pub(super) fn new(stream: S, operation_id: u64, reader: FrameReader) -> Self {
        Self {
            stream,
            operation_id,
            reader: Some(reader),
            write: None,
            readable: true,
            writable: true,
            write_first: false,
        }
    }

    /// Accept only the owner's exact Write for this connection, with no queue.
    /// A busy or mismatched port returns the entire unmodified effect.
    pub fn try_write(&mut self, effect: super::ClientResult) -> Result<(), super::ClientResult> {
        match effect {
            super::ClientResult::Write { operation_id, request_id, bytes }
                if operation_id == self.operation_id && self.write.is_none() =>
            {
                self.write = Some((request_id, bytes, 0));
                self.writable = true;
                Ok(())
            }
            other => Err(other),
        }
    }

    /// Merge actual device readiness. Neither hint changes client policy.
    pub fn ready(&mut self, readable: bool, writable: bool) {
        self.readable |= readable;
        self.writable |= writable;
    }

    /// Read/write interests for lanes blocked on the physical device. Runnable
    /// work is polled again after a fairness yield, without waiting for an edge.
    pub fn interests(&self) -> (bool, bool) {
        (self.reader.is_some() && !self.readable, self.write.is_some() && !self.writable)
    }

    /// `Ready(None)` means runnable progress, not an observation or completion.
    /// `Pending` means both lanes need readiness (or have no work); this method
    /// registers no waker. Deliver `Ready(Some(event))` to the sole owner before
    /// polling again. In particular retain a Read returned in DriveRejected;
    /// never discard it or substitute a freshly read frame after clock failure.
    pub fn poll(&mut self) -> Poll<Option<super::ClientEvent>> {
        let read = self.readable && self.reader.is_some();
        let write = self.writable && self.write.is_some();
        if write && (self.write_first || !read) {
            self.write_first = false;
            return self.poll_write();
        }
        if read {
            self.write_first = true;
            return self.poll_read();
        }
        Poll::Pending
    }

    fn runnable(&self) -> Poll<Option<super::ClientEvent>> {
        if (self.readable && self.reader.is_some()) || (self.writable && self.write.is_some()) {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }

    fn poll_read(&mut self) -> Poll<Option<super::ClientEvent>> {
        let Some(reader) = &mut self.reader else {
            return self.runnable();
        };
        let result = match reader.poll_bytes(&mut self.stream) {
            Ok(ReadProgress::Runnable) => return Poll::Ready(None),
            Ok(ReadProgress::WouldBlock) => {
                self.readable = false;
                return self.runnable();
            }
            Ok(ReadProgress::Frame(bytes)) => Ok(bytes),
            Ok(ReadProgress::Eof) => Err(ReadFailure::Io { source: io::ErrorKind::UnexpectedEof.into() }),
            Err(source) => Err(source),
        };
        if result.is_err() {
            // Report the actual failure once. Only the owner can demand Close;
            // the stream and any write buffer remain held until real disposal.
            self.reader = None;
            self.readable = false;
        }
        Poll::Ready(Some(super::ClientEvent::Read { operation_id: self.operation_id, result }))
    }

    fn poll_write(&mut self) -> Poll<Option<super::ClientEvent>> {
        let Some((request_id, bytes, offset)) = &mut self.write else {
            return self.runnable();
        };
        let offered = bytes.len() - *offset;
        let result = match self.stream.write(&bytes[*offset..]) {
            Ok(0) => Err(io::ErrorKind::WriteZero.into()),
            Ok(written) if written > offered => Err(io::ErrorKind::InvalidData.into()),
            Ok(written) => {
                *offset += written;
                if *offset < bytes.len() {
                    return Poll::Ready(None);
                }
                Ok(())
            }
            Err(source) if source.kind() == io::ErrorKind::Interrupted => return Poll::Ready(None),
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => {
                self.writable = false;
                return self.runnable();
            }
            Err(source) => Err(source),
        };
        let request_id = *request_id;
        self.write = None;
        Poll::Ready(Some(super::ClientEvent::Written { operation_id: self.operation_id, request_id, result }))
    }

    /// Dispose of the real stream AND partial buffers before returning Closed.
    /// This acknowledges physical retirement, not cancellation or non-admission.
    pub fn close(self) -> super::ClientEvent {
        let operation_id = self.operation_id;
        drop(self);
        super::ClientEvent::Closed { operation_id }
    }
}
