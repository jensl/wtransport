use crate::bytes::BufferReader;
use crate::bytes::BufferWriter;
use crate::bytes::BytesReader;
use crate::bytes::BytesWriter;
use crate::bytes::EndOfBuffer;
use crate::ids::InvalidSessionId;
use crate::ids::SessionId;
use crate::varint::VarInt;

#[cfg(feature = "async")]
use crate::bytes::AsyncRead;

#[cfg(feature = "async")]
use crate::bytes::AsyncWrite;

#[cfg(feature = "async")]
use crate::bytes::IoError;

/// Error stream header read operation.
#[derive(Debug)]
pub enum StreamHeaderReadError {
    /// Error for unknown stream type.
    UnknownStream,

    /// Error for invalid session ID.
    InvalidSessionId,
}

/// An error during async stream header read operation.
#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
#[derive(Debug)]
pub enum StreamHeaderReadAsyncError {
    /// Error during parsing stream header.
    StreamHeader(StreamHeaderReadError),

    /// Error due to I/O operation.
    IO(IoError),
}

#[cfg(feature = "async")]
impl From<IoError> for StreamHeaderReadAsyncError {
    fn from(io_error: IoError) -> Self {
        StreamHeaderReadAsyncError::IO(io_error)
    }
}

/// An HTTP3 stream type.
#[derive(Copy, Clone, Debug)]
pub enum StreamKind {
    /// CONTROL stream type.
    Control,

    /// QPACK Encoder stream type.
    QPackEncoder,

    /// QPACK Decoder stream type.
    QPackDecoder,

    /// WebTransport stream type.
    WebTransport,

    /// Exercise stream.
    Exercise(VarInt),
}

impl StreamKind {
    /// Checks whether an `id` is valid for a [`StreamKind::Exercise`].
    #[inline(always)]
    pub const fn is_id_exercise(id: VarInt) -> bool {
        id.into_inner() >= 0x21 && ((id.into_inner() - 0x21) % 0x1f == 0)
    }

    const fn parse(id: VarInt) -> Option<Self> {
        match id {
            stream_type_ids::CONTROL_STREAM => Some(StreamKind::Control),
            stream_type_ids::QPACK_ENCODER_STREAM => Some(StreamKind::QPackEncoder),
            stream_type_ids::QPACK_DECODER_STREAM => Some(StreamKind::QPackDecoder),
            stream_type_ids::WEBTRANSPORT_STREAM => Some(StreamKind::WebTransport),
            id if StreamKind::is_id_exercise(id) => Some(StreamKind::Exercise(id)),
            _ => None,
        }
    }

    const fn id(self) -> VarInt {
        match self {
            StreamKind::Control => stream_type_ids::CONTROL_STREAM,
            StreamKind::QPackEncoder => stream_type_ids::QPACK_ENCODER_STREAM,
            StreamKind::QPackDecoder => stream_type_ids::QPACK_DECODER_STREAM,
            StreamKind::WebTransport => stream_type_ids::WEBTRANSPORT_STREAM,
            StreamKind::Exercise(id) => id,
        }
    }
}

/// HTTP3 stream type.
///
/// *Unidirectional* HTTP3 streams have an header encoding the type.
pub struct StreamHeader {
    kind: StreamKind,
    session_id: Option<SessionId>,
}

impl StreamHeader {
    /// Maximum number of bytes a [`StreamHeader`] can take over network.
    pub const MAX_SIZE: usize = 16;

    /// Creates a new stream header of type [`StreamKind::Control`].
    #[inline(always)]
    pub fn new_control() -> Self {
        Self::new(StreamKind::Control, None)
    }

    /// Creates a new stream header of type [`StreamKind::WebTransport`].
    #[inline(always)]
    pub fn new_webtransport(session_id: SessionId) -> Self {
        Self::new(StreamKind::WebTransport, Some(session_id))
    }

    /// Reads a [`StreamHeader`] from a [`BytesReader`].
    ///
    /// It returns [`None`] if the `bytes_reader` does not contain enough bytes
    /// to parse an entire header.
    ///
    /// In case [`None`] or [`Err`], `bytes_reader` might be partially read.
    pub fn read<'a, R>(bytes_reader: &mut R) -> Option<Result<Self, StreamHeaderReadError>>
    where
        R: BytesReader<'a>,
    {
        let kind_id = bytes_reader.get_varint()?;
        let kind = match StreamKind::parse(kind_id) {
            Some(kind) => kind,
            None => return Some(Err(StreamHeaderReadError::UnknownStream)),
        };

        let session_id = if matches!(kind, StreamKind::WebTransport) {
            let session_id = match SessionId::try_from_varint(bytes_reader.get_varint()?) {
                Ok(session_id) => session_id,
                Err(InvalidSessionId) => return Some(Err(StreamHeaderReadError::InvalidSessionId)),
            };

            Some(session_id)
        } else {
            None
        };

        Some(Ok(Self::new(kind, session_id)))
    }

    /// Reads a [`StreamHeader`] from a `reader`.
    #[cfg(feature = "async")]
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub async fn read_async<R>(reader: &mut R) -> Result<Self, StreamHeaderReadAsyncError>
    where
        R: AsyncRead + Unpin + ?Sized,
    {
        use crate::bytes::BytesReaderAsync;

        let kind_id = reader.get_varint().await?;
        let kind = StreamKind::parse(kind_id).ok_or(StreamHeaderReadAsyncError::StreamHeader(
            StreamHeaderReadError::UnknownStream,
        ))?;

        let session_id = if matches!(kind, StreamKind::WebTransport) {
            let session_id = SessionId::try_from_varint(reader.get_varint().await?).map_err(
                |InvalidSessionId| {
                    StreamHeaderReadAsyncError::StreamHeader(
                        StreamHeaderReadError::InvalidSessionId,
                    )
                },
            )?;

            Some(session_id)
        } else {
            None
        };

        Ok(Self::new(kind, session_id))
    }

    /// Reads a [`StreamHeader`] from a [`BufferReader`].
    ///
    /// It returns [`None`] if the `buffer_reader` does not contain enough bytes
    /// to parse an entire header.
    ///
    /// In case [`None`] or [`Err`], `buffer_reader` offset if not advanced.
    pub fn read_from_buffer(
        buffer_reader: &mut BufferReader,
    ) -> Option<Result<Self, StreamHeaderReadError>> {
        let mut buffer_reader_child = buffer_reader.child();

        match Self::read(&mut *buffer_reader_child)? {
            Ok(header) => {
                buffer_reader_child.commit();
                Some(Ok(header))
            }
            Err(error) => Some(Err(error)),
        }
    }

    /// Writes a [`StreamHeader`] into a [`BytesWriter`].
    ///
    /// It returns [`Err`] if the `bytes_writer` does not have enough capacity
    /// to write the entire header.
    /// See [`Self::write_size`] to retrieve the extact amount of required capacity.
    ///
    /// In case [`Err`], `bytes_writer` might be partially written.
    pub fn write<W>(&self, bytes_writer: &mut W) -> Result<(), EndOfBuffer>
    where
        W: BytesWriter,
    {
        bytes_writer.put_varint(self.kind.id())?;

        if let Some(session_id) = self.session_id() {
            bytes_writer.put_varint(session_id.into_varint())?;
        }

        Ok(())
    }

    /// Writes a [`StreamHeader`] into a `writer`.
    #[cfg(feature = "async")]
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub async fn write_async<W>(&self, writer: &mut W) -> Result<(), IoError>
    where
        W: AsyncWrite + Unpin + ?Sized,
    {
        use crate::bytes::BytesWriterAsync;

        writer.put_varint(self.kind.id()).await?;

        if let Some(session_id) = self.session_id() {
            writer.put_varint(session_id.into_varint()).await?;
        }

        Ok(())
    }

    /// Writes this [`StreamHeader`] into a buffer via [`BufferWriter`].
    ///
    /// In case [`Err`], `buffer_writer` is not advanced.
    pub fn write_to_buffer(&self, buffer_writer: &mut BufferWriter) -> Result<(), EndOfBuffer> {
        if buffer_writer.capacity() < self.write_size() {
            return Err(EndOfBuffer);
        }

        self.write(buffer_writer)
            .expect("Enough capacity for header");

        Ok(())
    }

    /// Returns the needed capacity to write this stream header into a buffer.
    pub fn write_size(&self) -> usize {
        if let Some(session_id) = self.session_id() {
            self.kind.id().size() + session_id.into_varint().size()
        } else {
            self.kind.id().size()
        }
    }

    /// Returns the [`StreamKind`].
    #[inline(always)]
    pub const fn kind(&self) -> StreamKind {
        self.kind
    }

    /// Returns the [`SessionId`] if stream is [`StreamKind::WebTransport`],
    /// otherwise returns [`None`].
    #[inline(always)]
    pub fn session_id(&self) -> Option<SessionId> {
        matches!(self.kind, StreamKind::WebTransport).then(|| {
            self.session_id
                .expect("WebTransport stream header contains session id")
        })
    }

    fn new(kind: StreamKind, session_id: Option<SessionId>) -> Self {
        if let StreamKind::Exercise(id) = kind {
            debug_assert!(StreamKind::is_id_exercise(id))
        } else if let StreamKind::WebTransport = kind {
            debug_assert!(session_id.is_some())
        }

        Self { kind, session_id }
    }
}

mod stream_type_ids {
    use crate::varint::VarInt;

    pub const CONTROL_STREAM: VarInt = VarInt::from_u32(0x0);
    pub const QPACK_ENCODER_STREAM: VarInt = VarInt::from_u32(0x02);
    pub const QPACK_DECODER_STREAM: VarInt = VarInt::from_u32(0x03);
    pub const WEBTRANSPORT_STREAM: VarInt = VarInt::from_u32(0x54);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control() {
        let stream_header = StreamHeader::new_control();
        assert!(matches!(stream_header.kind(), StreamKind::Control));
        assert!(stream_header.session_id().is_none());

        let stream_header = utils::assert_serde(stream_header);
        assert!(matches!(stream_header.kind(), StreamKind::Control));
        assert!(stream_header.session_id().is_none());
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn control_async() {
        let stream_header = StreamHeader::new_control();
        assert!(matches!(stream_header.kind(), StreamKind::Control));
        assert!(stream_header.session_id().is_none());

        let stream_header = utils::assert_serde_async(stream_header).await;
        assert!(matches!(stream_header.kind(), StreamKind::Control));
        assert!(stream_header.session_id().is_none());
    }

    #[test]
    fn webtransport() {
        let session_id = SessionId::try_from_varint(VarInt::from_u32(0)).unwrap();

        let stream_header = StreamHeader::new_webtransport(session_id);
        assert!(matches!(stream_header.kind(), StreamKind::WebTransport));
        assert!(matches!(stream_header.session_id(), Some(x) if x == session_id));

        let stream_header = utils::assert_serde(stream_header);
        assert!(matches!(stream_header.kind(), StreamKind::WebTransport));
        assert!(matches!(stream_header.session_id(), Some(x) if x == session_id));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn webtransport_async() {
        let session_id = SessionId::try_from_varint(VarInt::from_u32(0)).unwrap();

        let stream_header = StreamHeader::new_webtransport(session_id);
        assert!(matches!(stream_header.kind(), StreamKind::WebTransport));
        assert!(matches!(stream_header.session_id(), Some(x) if x == session_id));

        let stream_header = utils::assert_serde_async(stream_header).await;
        assert!(matches!(stream_header.kind(), StreamKind::WebTransport));
        assert!(matches!(stream_header.session_id(), Some(x) if x == session_id));
    }

    #[test]
    fn read_eof() {
        let mut buffer = Vec::new();
        StreamHeader::new_control().write(&mut buffer).unwrap();
        assert!(StreamHeader::read(&mut &buffer[..buffer.len() - 1]).is_none());
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn read_eof_async() {
        let mut buffer = Vec::new();
        StreamHeader::new_control().write(&mut buffer).unwrap();

        assert!(matches!(
            StreamHeader::read_async(&mut &buffer[..buffer.len() - 1]).await,
            Err(StreamHeaderReadAsyncError::IO(IoError::Closed))
        ));
    }

    #[test]
    fn unknown_stream() {
        let mut buffer = Vec::new();

        StreamHeader {
            kind: StreamKind::Exercise(VarInt::from_u32(0x42)),
            session_id: None,
        }
        .write(&mut buffer)
        .unwrap();

        assert!(matches!(
            StreamHeader::read(&mut buffer.as_slice()).unwrap(),
            Err(StreamHeaderReadError::UnknownStream)
        ));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn unknown_fame_async() {
        let mut buffer = Vec::new();

        StreamHeader {
            kind: StreamKind::Exercise(VarInt::from_u32(0x42)),
            session_id: None,
        }
        .write(&mut buffer)
        .unwrap();

        assert!(matches!(
            StreamHeader::read_async(&mut buffer.as_slice()).await,
            Err(StreamHeaderReadAsyncError::StreamHeader(
                StreamHeaderReadError::UnknownStream
            ))
        ));
    }

    #[test]
    fn invalid_session_id() {
        let mut buffer = Vec::new();

        let invalid_session_id = SessionId::maybe_invalid(VarInt::from_u32(1));

        StreamHeader {
            kind: StreamKind::WebTransport,
            session_id: Some(invalid_session_id),
        }
        .write(&mut buffer)
        .unwrap();

        assert!(matches!(
            StreamHeader::read(&mut buffer.as_slice()).unwrap(),
            Err(StreamHeaderReadError::InvalidSessionId)
        ));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn invalid_session_id_async() {
        let mut buffer = Vec::new();

        let invalid_session_id = SessionId::maybe_invalid(VarInt::from_u32(1));

        StreamHeader {
            kind: StreamKind::WebTransport,
            session_id: Some(invalid_session_id),
        }
        .write(&mut buffer)
        .unwrap();

        assert!(matches!(
            StreamHeader::read_async(&mut buffer.as_slice()).await,
            Err(StreamHeaderReadAsyncError::StreamHeader(
                StreamHeaderReadError::InvalidSessionId
            ))
        ));
    }

    mod utils {
        use super::*;

        pub fn assert_serde(stream_header: StreamHeader) -> StreamHeader {
            let mut buffer = Vec::new();

            stream_header.write(&mut buffer).unwrap();
            assert_eq!(buffer.len(), stream_header.write_size());
            assert!(buffer.len() <= StreamHeader::MAX_SIZE);

            let mut buffer = buffer.as_slice();
            let stream_header = StreamHeader::read(&mut buffer).unwrap().unwrap();
            assert!(buffer.is_empty());

            stream_header
        }

        #[cfg(feature = "async")]
        pub async fn assert_serde_async(stream_header: StreamHeader) -> StreamHeader {
            let mut buffer = Vec::new();

            stream_header.write_async(&mut buffer).await.unwrap();
            assert_eq!(buffer.len(), stream_header.write_size());
            assert!(buffer.len() <= StreamHeader::MAX_SIZE);

            let mut buffer = buffer.as_slice();
            let stream_header = StreamHeader::read_async(&mut buffer).await.unwrap();
            assert!(buffer.is_empty());

            stream_header
        }
    }
}
