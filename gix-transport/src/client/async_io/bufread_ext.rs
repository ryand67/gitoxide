use std::{
    io,
    ops::{Deref, DerefMut},
};

use async_trait::async_trait;
use futures_io::{AsyncBufRead, AsyncRead};
use gix_packetline::PacketLineRef;

use crate::{
    client::{Error, MessageKind},
    Protocol,
};

/// A function `f(is_error, text)` receiving progress or error information.
/// As it is not a future itself, it must not block. If IO is performed within the function, be sure to spawn
/// it onto an executor.
pub type HandleProgress = Box<dyn FnMut(bool, &[u8])>;

/// This trait exists to get a version of a `gix_packetline::Provider` without type parameters,
/// but leave support for reading lines directly without forcing them through `String`.
///
/// For the sake of usability, it also implements [`std::io::BufRead`] making it trivial to
/// read pack files while keeping open the option to read individual lines with low overhead.
#[async_trait(?Send)]
pub trait ReadlineBufRead: AsyncBufRead {
    /// Read a packet line into the internal buffer and return it.
    ///
    /// Returns `None` if the end of iteration is reached because of one of the following:
    ///
    ///  * natural EOF
    ///  * ERR packet line encountered
    ///  * A `delimiter` packet line encountered
    async fn readline(
        &mut self,
    ) -> Option<io::Result<Result<gix_packetline::PacketLineRef<'_>, gix_packetline::decode::Error>>>;
}

/// Provide even more access to the underlying packet reader.
#[async_trait(?Send)]
pub trait ExtendedBufRead: ReadlineBufRead {
    /// Set the handler to which progress will be delivered.
    ///
    /// Note that this is only possible if packet lines are sent in side band mode.
    fn set_progress_handler(&mut self, handle_progress: Option<HandleProgress>);
    /// Peek the next data packet line. Maybe None if the next line is a packet we stop at, queryable using
    /// [`stopped_at()`][ExtendedBufRead::stopped_at()].
    async fn peek_data_line(&mut self) -> Option<io::Result<Result<&[u8], Error>>>;
    /// Resets the reader to allow reading past a previous stop, and sets delimiters according to the
    /// given protocol.
    fn reset(&mut self, version: Protocol);
    /// Return the kind of message at which the reader stopped.
    fn stopped_at(&self) -> Option<MessageKind>;
}

#[async_trait(?Send)]
impl<'a, T: ReadlineBufRead + ?Sized + 'a + Unpin> ReadlineBufRead for Box<T> {
    async fn readline(&mut self) -> Option<io::Result<Result<PacketLineRef<'_>, gix_packetline::decode::Error>>> {
        self.deref_mut().readline().await
    }
}

#[async_trait(?Send)]
impl<'a, T: ExtendedBufRead + ?Sized + 'a + Unpin> ExtendedBufRead for Box<T> {
    fn set_progress_handler(&mut self, handle_progress: Option<HandleProgress>) {
        self.deref_mut().set_progress_handler(handle_progress)
    }

    async fn peek_data_line(&mut self) -> Option<io::Result<Result<&[u8], Error>>> {
        self.deref_mut().peek_data_line().await
    }

    fn reset(&mut self, version: Protocol) {
        self.deref_mut().reset(version)
    }

    fn stopped_at(&self) -> Option<MessageKind> {
        self.deref().stopped_at()
    }
}

#[async_trait(?Send)]
impl<T: AsyncRead + Unpin> ReadlineBufRead for gix_packetline::read::WithSidebands<'_, T, for<'b> fn(bool, &'b [u8])> {
    async fn readline(&mut self) -> Option<io::Result<Result<PacketLineRef<'_>, gix_packetline::decode::Error>>> {
        self.read_data_line().await
    }
}

#[async_trait(?Send)]
impl<'a, T: AsyncRead + Unpin> ReadlineBufRead for gix_packetline::read::WithSidebands<'a, T, HandleProgress> {
    async fn readline(&mut self) -> Option<io::Result<Result<PacketLineRef<'_>, gix_packetline::decode::Error>>> {
        self.read_data_line().await
    }
}

#[async_trait(?Send)]
impl<'a, T: AsyncRead + Unpin> ExtendedBufRead for gix_packetline::read::WithSidebands<'a, T, HandleProgress> {
    fn set_progress_handler(&mut self, handle_progress: Option<HandleProgress>) {
        self.set_progress_handler(handle_progress)
    }
    async fn peek_data_line(&mut self) -> Option<io::Result<Result<&[u8], Error>>> {
        match self.peek_data_line().await {
            Some(Ok(Ok(line))) => Some(Ok(Ok(line))),
            Some(Ok(Err(err))) => Some(Ok(Err(err.into()))),
            Some(Err(err)) => Some(Err(err)),
            None => None,
        }
    }
    fn reset(&mut self, version: Protocol) {
        match version {
            Protocol::V1 => self.reset_with(&[gix_packetline::PacketLineRef::Flush]),
            Protocol::V2 => self.reset_with(&[
                gix_packetline::PacketLineRef::Delimiter,
                gix_packetline::PacketLineRef::Flush,
            ]),
        }
    }
    fn stopped_at(&self) -> Option<MessageKind> {
        self.stopped_at().map(|l| match l {
            gix_packetline::PacketLineRef::Flush => MessageKind::Flush,
            gix_packetline::PacketLineRef::Delimiter => MessageKind::Delimiter,
            gix_packetline::PacketLineRef::ResponseEnd => MessageKind::ResponseEnd,
            gix_packetline::PacketLineRef::Data(_) => unreachable!("data cannot be a delimiter"),
        })
    }
}
