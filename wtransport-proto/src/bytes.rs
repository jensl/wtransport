use crate::varint::VarInt;
use octets::Octets;
use octets::OctetsMut;
use std::ops::Deref;
use std::ops::DerefMut;

/// An error indicating write operation was not able to complete because
/// end of buffer has been reached.
#[derive(Debug)]
pub struct EndOfBuffer;

/// Reads bytes or varint from a source.
pub trait BytesReader<'a> {
    /// Reads an unsigned variable-length integer in network byte-order from
    /// the current offset and advances the offset.
    ///
    /// Returns [`None`] if not enough capacity (offset is not advanced in that case).
    fn get_varint(&mut self) -> Option<VarInt>;

    /// Reads `len` bytes from the current offset without copying and advances
    /// the offset.
    ///
    /// Returns [`None`] if not enough capacity (offset is not advanced in that case).
    fn get_bytes(&mut self, len: usize) -> Option<&'a [u8]>;
}

impl<'a> BytesReader<'a> for &'a [u8] {
    fn get_varint(&mut self) -> Option<VarInt> {
        let varint_size = VarInt::parse_size(*self.first()?);
        let buffer = self.get(..varint_size)?;
        let varint = BufferReader::new(buffer)
            .get_varint()
            .expect("Varint parsable");
        *self = &self[varint_size..];
        Some(varint)
    }

    fn get_bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        let buffer = self.get(..len)?;
        *self = &self[len..];
        Some(buffer)
    }
}

/// Writes bytes or varint on a source.
pub trait BytesWriter {
    /// Writes an unsigned variable-length integer in network byte-order at the
    /// current offset and advances the offset.
    ///
    /// Returns [`Err`] if source is exhausted and no space is available.
    fn put_varint(&mut self, varint: VarInt) -> Result<(), EndOfBuffer>;

    /// Writes (by **copy**) all `bytes` at the current offset and advances it.
    ///
    /// Returns [`Err`] if source is exhausted and no space is available.
    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), EndOfBuffer>;
}

impl BytesWriter for Vec<u8> {
    fn put_varint(&mut self, varint: VarInt) -> Result<(), EndOfBuffer> {
        let offset = self.len();

        self.resize(offset + varint.size(), 0);

        BufferWriter::new(&mut self[offset..])
            .put_varint(varint)
            .expect("Enough capacity prellocated");

        Ok(())
    }

    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), EndOfBuffer> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

/// A zero-copy immutable byte-buffer reader.
///
/// Internally, it stores an offset that is increased during reading.
pub struct BufferReader<'a>(Octets<'a>);

impl<'a> BufferReader<'a> {
    /// Creates a [`BufferReader`] from the given slice, without copying.
    ///
    /// Inner offset is initialized to zero.
    #[inline(always)]
    pub fn new(buffer: &'a [u8]) -> Self {
        Self(Octets::with_slice(buffer))
    }

    /// Returns the remaining capacity in the buffer.
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.0.cap()
    }

    /// Returns the current offset of the buffer.
    #[inline(always)]
    pub fn offset(&self) -> usize {
        self.0.off()
    }

    /// Advances the offset.
    ///
    /// In case of [`Err`] the offset is not advanced.
    #[inline(always)]
    pub fn skip(&mut self, len: usize) -> Result<(), EndOfBuffer> {
        self.0
            .skip(len)
            .map_err(|octets::BufferTooShortError| EndOfBuffer)
    }

    /// Returns a reference to the internal buffer.
    ///
    /// **Note**: this is the entire buffer (despite offset).
    #[inline(always)]
    pub fn buffer(&self) -> &'a [u8] {
        self.0.buf()
    }

    /// Returns the inner buffer starting from the current offset.
    #[inline(always)]
    pub fn buffer_remaining(&mut self) -> &'a [u8] {
        &self.buffer()[self.offset()..]
    }

    /// Creates a [`BufferReaderChild`] with this parent.
    #[inline(always)]
    pub fn child(&mut self) -> BufferReaderChild<'a, '_> {
        BufferReaderChild::with_parent(self)
    }
}

impl<'a> BytesReader<'a> for BufferReader<'a> {
    #[inline(always)]
    fn get_varint(&mut self) -> Option<VarInt> {
        match self.0.get_varint() {
            Ok(value) => {
                // SAFETY: octets returns a legit varint
                Some(unsafe {
                    debug_assert!(value <= VarInt::MAX.into_inner());
                    VarInt::from_u64_unchecked(value)
                })
            }
            Err(octets::BufferTooShortError) => None,
        }
    }

    #[inline(always)]
    fn get_bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        self.0.get_bytes(len).ok().map(|o| o.buf())
    }
}

/// It acts like a copy of a parent [`BufferReader`].
///
/// You can create this from [`BufferReader::child`].
///
/// Having a copy it allows reading the buffer preserving the parent's original offset.
///
/// If you want to commit the number of bytes read to the parent, use [`BufferReaderChild::commit`].
/// Instead, dropping this without committing, it will not alter the parent.
pub struct BufferReaderChild<'a, 'b> {
    reader: BufferReader<'a>,
    parent: &'b mut BufferReader<'a>,
}

impl<'a, 'b> BufferReaderChild<'a, 'b> {
    /// Advances the parent [`BufferReader`] offset of the amount read on this child.
    #[inline(always)]
    pub fn commit(self) {
        self.parent
            .skip(self.reader.offset())
            .expect("Child offset is bounded to parent")
    }

    #[inline(always)]
    fn with_parent(parent: &'b mut BufferReader<'a>) -> Self {
        Self {
            reader: BufferReader::new(parent.buffer_remaining()),
            parent,
        }
    }
}

impl<'a, 'b> Deref for BufferReaderChild<'a, 'b> {
    type Target = BufferReader<'a>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

impl<'a, 'b> DerefMut for BufferReaderChild<'a, 'b> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.reader
    }
}

/// A zero-copy mutable buffer writer.
pub struct BufferWriter<'a>(OctetsMut<'a>);

impl<'a> BufferWriter<'a> {
    /// Creates an [`BufferWriter`] by using `bytes` as inner buffer.
    ///
    /// Inner offset is initialized to zero.
    #[inline(always)]
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self(OctetsMut::with_slice(bytes))
    }

    /// Returns the remaining capacity in the buffer.
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.0.cap()
    }

    /// Returns the current offset of the buffer.
    #[inline(always)]
    pub fn offset(&self) -> usize {
        self.0.off()
    }

    /// Returns the portion of the inner buffer written so far.
    #[inline(always)]
    pub fn buffer_written(&self) -> &[u8] {
        &self.0.buf()[..self.offset()]
    }
}

impl<'a> BytesWriter for BufferWriter<'a> {
    #[inline(always)]
    fn put_varint(&mut self, varint: VarInt) -> Result<(), EndOfBuffer> {
        self.0
            .put_varint(varint.into_inner())
            .map_err(|octets::BufferTooShortError| EndOfBuffer)?;

        Ok(())
    }

    #[inline(always)]
    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), EndOfBuffer> {
        self.0
            .put_bytes(bytes)
            .map_err(|octets::BufferTooShortError| EndOfBuffer)
    }
}

/// Async operations.
#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod r#async {
    use super::*;
    use std::future::Future;
    use std::io::ErrorKind as IoErrorKind;
    use std::io::Result as IoResult;
    use std::pin::Pin;
    use std::task::ready;
    use std::task::Context;
    use std::task::Poll;

    /// Error during I/O async operation.
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    #[derive(Debug)]
    pub enum IoError {
        /// The operation failed because it was not connected yet.
        NotConnected,

        /// The operation did not completed because underlying transport was closed.
        Closed,
    }

    impl From<std::io::Error> for IoError {
        fn from(error: std::io::Error) -> Self {
            match error.kind() {
                IoErrorKind::NotConnected => IoError::NotConnected,
                _ => IoError::Closed,
            }
        }
    }

    /// Reads bytes from a source.
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub trait AsyncRead {
        /// Attempt to read from the source **copying** into `buf`.
        ///
        /// * On success, returns `Poll::Ready(num_bytes_read)`.
        /// * It returns `0` (i.e., `num_bytes_read == 0`) if:
        ///   - `buf.len() == 0` (output buffer is empty);
        ///   - EOF.
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<IoResult<usize>>;
    }

    /// Writes bytes into a source.
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub trait AsyncWrite {
        /// Attempt to write bytes into the source copying from `buf`.
        ///
        /// On success, returns `Poll::Ready(num_bytes_written)`.
        ///
        /// **Note**: this should never return `Ok(0)` if `buf.len() > 0`.
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<IoResult<usize>>;
    }

    /// Reads bytes or varints asynchronously.
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub trait BytesReaderAsync {
        /// Reads an unsigned variable-length integer in network byte-order from
        /// the source advancing the buffer's internal cursor.
        fn get_varint(&mut self) -> GetVarint<Self>;

        /// Pulls some bytes from this source into the specified buffer
        /// advancing the buffer’s internal cursor.
        fn get_buffer<'a>(&'a mut self, buffer: &'a mut [u8]) -> GetBuffer<Self>;
    }

    impl<T> BytesReaderAsync for T
    where
        T: AsyncRead + ?Sized,
    {
        fn get_varint(&mut self) -> GetVarint<Self> {
            GetVarint::new(self)
        }

        fn get_buffer<'a>(&'a mut self, buffer: &'a mut [u8]) -> GetBuffer<Self> {
            GetBuffer::new(self, buffer)
        }
    }

    impl<T> BytesWriterAsync for T
    where
        T: AsyncWrite + ?Sized,
    {
        fn put_varint(&mut self, varint: VarInt) -> PutVarint<Self> {
            PutVarint::new(self, varint)
        }

        fn put_buffer<'a>(&'a mut self, buffer: &'a [u8]) -> PutBuffer<Self> {
            PutBuffer::new(self, buffer)
        }
    }

    /// Writes bytes or varints asynchronously.
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub trait BytesWriterAsync {
        /// Writes an unsigned variable-length integer in network byte-order to
        /// the source advancing the buffer's internal cursor.
        ///
        /// On succes, return the number of bytes written (the varint lenght of encoded value).
        fn put_varint(&mut self, varint: VarInt) -> PutVarint<Self>;

        /// Pushes some bytes into ths source advancing the buffer’s internal cursor.
        fn put_buffer<'a>(&'a mut self, buffer: &'a [u8]) -> PutBuffer<Self>;
    }

    /// [`Future`] for reading a varint.
    ///
    /// Created by [`BytesReaderAsync::get_varint`].
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub struct GetVarint<'a, R: ?Sized> {
        reader: &'a mut R,
        buffer: [u8; VarInt::MAX_SIZE],
        offset: usize,
        varint_size: usize,
    }

    impl<'a, R> GetVarint<'a, R>
    where
        R: AsyncRead + ?Sized,
    {
        fn new(reader: &'a mut R) -> Self {
            Self {
                reader,
                buffer: [0; VarInt::MAX_SIZE],
                offset: 0,
                varint_size: 0,
            }
        }
    }

    impl<'a, R> Future for GetVarint<'a, R>
    where
        R: AsyncRead + Unpin + ?Sized,
    {
        type Output = Result<VarInt, IoError>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();

            if this.offset == 0 {
                debug_assert_eq!(this.varint_size, 0);

                let read = ready!(AsyncRead::poll_read(
                    Pin::new(this.reader),
                    cx,
                    &mut this.buffer[0..1]
                ))?;

                debug_assert!(read == 0 || read == 1);

                if read == 1 {
                    this.offset = 1;
                    this.varint_size = VarInt::parse_size(this.buffer[0]);
                    debug_assert_ne!(this.varint_size, 0);
                } else {
                    return Poll::Ready(Err(IoError::Closed));
                }
            }

            while this.offset < this.varint_size {
                let read = ready!(AsyncRead::poll_read(
                    Pin::new(this.reader),
                    cx,
                    &mut this.buffer[this.offset..this.varint_size]
                ))?;

                if read > 0 {
                    this.offset += read;
                } else {
                    return Poll::Ready(Err(IoError::Closed));
                }
            }

            let varint = BufferReader::new(&this.buffer[..this.varint_size])
                .get_varint()
                .expect("Varint is parsable");

            Poll::Ready(Ok(varint))
        }
    }

    /// [`Future`] for reading a buffer of bytes.
    ///
    /// Created by [`BytesReaderAsync::get_buffer`].
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub struct GetBuffer<'a, R: ?Sized> {
        reader: &'a mut R,
        buffer: &'a mut [u8],
        offset: usize,
    }

    impl<'a, R> GetBuffer<'a, R>
    where
        R: AsyncRead + ?Sized,
    {
        fn new(reader: &'a mut R, buffer: &'a mut [u8]) -> Self {
            Self {
                reader,
                buffer,
                offset: 0,
            }
        }
    }

    impl<'a, R> Future for GetBuffer<'a, R>
    where
        R: AsyncRead + Unpin + ?Sized,
    {
        type Output = Result<(), IoError>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();

            while this.offset < this.buffer.len() {
                let read = ready!(AsyncRead::poll_read(
                    Pin::new(this.reader),
                    cx,
                    &mut this.buffer[this.offset..],
                ))?;

                if read > 0 {
                    this.offset += read;
                } else {
                    return Poll::Ready(Err(IoError::Closed));
                }
            }

            Poll::Ready(Ok(()))
        }
    }

    /// [`Future`] for writing a varint.
    ///
    /// Created by [`BytesWriterAsync::put_varint`].
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub struct PutVarint<'a, W: ?Sized> {
        writer: &'a mut W,
        buffer: [u8; VarInt::MAX_SIZE],
        offset: usize,
        varint_size: usize,
    }

    impl<'a, W> PutVarint<'a, W>
    where
        W: AsyncWrite + ?Sized,
    {
        fn new(writer: &'a mut W, varint: VarInt) -> Self {
            let mut this = Self {
                writer,
                buffer: [0; VarInt::MAX_SIZE],
                offset: 0,
                varint_size: 0,
            };

            let mut buffer_writer = BufferWriter::new(&mut this.buffer);
            buffer_writer
                .put_varint(varint)
                .expect("Inner buffer is enough for max varint");

            this.varint_size = buffer_writer.offset();

            this
        }
    }

    impl<'a, W> Future for PutVarint<'a, W>
    where
        W: AsyncWrite + Unpin + ?Sized,
    {
        type Output = Result<usize, IoError>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();

            while this.offset < this.varint_size {
                let written = ready!(AsyncWrite::poll_write(
                    Pin::new(this.writer),
                    cx,
                    &this.buffer[this.offset..this.varint_size]
                ))?;

                // TODO(bfesta): what if AsyncWrite returns Ok(0)? maybe wake and pending?
                debug_assert!(written > 0);

                this.offset += written;
            }

            Poll::Ready(Ok(this.varint_size))
        }
    }

    /// [`Future`] for writing a buffer of bytes.
    ///
    /// Created by [`BytesWriterAsync::put_buffer`].
    #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
    pub struct PutBuffer<'a, W: ?Sized> {
        writer: &'a mut W,
        buffer: &'a [u8],
        offset: usize,
    }

    impl<'a, W> PutBuffer<'a, W>
    where
        W: AsyncWrite + ?Sized,
    {
        fn new(writer: &'a mut W, buffer: &'a [u8]) -> Self {
            Self {
                writer,
                buffer,
                offset: 0,
            }
        }
    }

    impl<'a, W> Future for PutBuffer<'a, W>
    where
        W: AsyncWrite + Unpin + ?Sized,
    {
        type Output = Result<(), IoError>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();

            while this.offset < this.buffer.len() {
                let written = ready!(AsyncWrite::poll_write(
                    Pin::new(this.writer),
                    cx,
                    &this.buffer[this.offset..]
                ))?;

                // TODO(bfesta): what if AsyncWrite returns Ok(0)? maybe wake and pending?
                debug_assert!(written > 0);

                this.offset += written;
            }

            Poll::Ready(Ok(()))
        }
    }

    // TODO(bfesta): move this under trait def. same for below writer
    impl AsyncRead for &[u8] {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<IoResult<usize>> {
            let amt = std::cmp::min(self.len(), buf.len());
            let (a, b) = self.split_at(amt);

            if !a.is_empty() {
                buf.copy_from_slice(a);
            }

            *self = b;

            Poll::Ready(Ok(amt))
        }
    }

    impl AsyncWrite for Vec<u8> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<IoResult<usize>> {
            self.extend_from_slice(buf);
            Poll::Ready(IoResult::Ok(buf.len()))
        }
    }
}

#[cfg(feature = "async")]
pub use r#async::AsyncRead;

#[cfg(feature = "async")]
pub use r#async::AsyncWrite;

#[cfg(feature = "async")]
pub use r#async::BytesReaderAsync;

#[cfg(feature = "async")]
pub use r#async::BytesWriterAsync;

#[cfg(feature = "async")]
pub use r#async::IoError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_varint() {
        for (varint_buffer, value_expect) in utils::VARINT_TEST_CASES {
            let mut buffer_reader = BufferReader::new(varint_buffer);

            assert_eq!(buffer_reader.offset(), 0);
            assert_eq!(buffer_reader.capacity(), varint_buffer.len());

            let value = buffer_reader.get_varint().unwrap();

            assert_eq!(value, value_expect);
            assert_eq!(buffer_reader.offset(), varint_buffer.len());
            assert_eq!(buffer_reader.capacity(), 0);
        }
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn parse_varint_async() {
        for (varint_buffer, value_expect) in utils::VARINT_TEST_CASES {
            let mut reader = utils::StepReader::new(varint_buffer);
            let value = reader.get_varint().await.unwrap();
            assert_eq!(value, value_expect);
        }
    }

    #[test]
    fn write_varint() {
        let mut buffer = [0; VarInt::MAX_SIZE];
        for (varint_buffer, value) in utils::VARINT_TEST_CASES {
            let mut buffer_writer = BufferWriter::new(&mut buffer);

            assert_eq!(buffer_writer.offset(), 0);
            assert_eq!(buffer_writer.capacity(), VarInt::MAX_SIZE);

            buffer_writer.put_varint(value).unwrap();

            assert_eq!(buffer_writer.offset(), varint_buffer.len());
            assert_eq!(buffer_writer.buffer_written(), varint_buffer);
        }
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn write_varint_async() {
        for (varint_buffer, value) in utils::VARINT_TEST_CASES {
            let mut writer = utils::StepWriter::new(Some(8));

            let varint_len = writer.put_varint(value).await.unwrap();
            assert_eq!(varint_len, writer.written().len());
            assert_eq!(writer.written(), varint_buffer);
        }
    }

    #[test]
    fn child_commit() {
        let mut buffer_reader = BufferReader::new(&[0x1, 0x2]);

        buffer_reader.skip(1).unwrap();
        assert_eq!(buffer_reader.offset(), 1);
        assert_eq!(buffer_reader.capacity(), 1);

        let mut buffer_reader_child = buffer_reader.child();
        assert_eq!(buffer_reader_child.offset(), 0);
        assert_eq!(buffer_reader_child.capacity(), 1);

        assert!(matches!(buffer_reader_child.get_bytes(1), Some([0x2])));
        assert_eq!(buffer_reader_child.offset(), 1);

        buffer_reader_child.commit();

        assert_eq!(buffer_reader.offset(), 2);
        assert_eq!(buffer_reader.capacity(), 0);
    }

    #[test]
    fn child_drop() {
        let mut buffer_reader = BufferReader::new(&[0x1, 0x2]);

        buffer_reader.skip(1).unwrap();
        assert_eq!(buffer_reader.offset(), 1);
        assert_eq!(buffer_reader.capacity(), 1);

        let mut buffer_reader_child = buffer_reader.child();
        assert_eq!(buffer_reader_child.offset(), 0);
        assert_eq!(buffer_reader_child.capacity(), 1);

        assert!(matches!(buffer_reader_child.get_bytes(1), Some([0x2])));
        assert_eq!(buffer_reader_child.offset(), 1);

        assert_eq!(buffer_reader.offset(), 1);
        assert_eq!(buffer_reader.capacity(), 1);
    }

    #[test]
    fn none() {
        let mut buffer_reader = BufferReader::new(&[]);
        assert!(matches!(buffer_reader.get_varint(), None));
        assert!(matches!(buffer_reader.get_bytes(1), None));

        let mut buffer_writer = BufferWriter::new(&mut []);
        assert!(buffer_writer.put_varint(VarInt::from_u32(0)).is_err());
        assert!(buffer_writer.put_bytes(&[0x0]).is_err());
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn none_async() {
        let mut reader = utils::StepReader::new(vec![]);
        assert!(reader.get_varint().await.is_err());
        assert!(reader.get_buffer(&mut [0x0]).await.is_err());

        let mut writer = utils::StepWriter::new(Some(0));
        assert!(writer.put_varint(VarInt::from_u32(0)).await.is_err());
        assert!(writer.put_buffer(&[0x0]).await.is_err());
    }

    mod utils {
        use super::*;

        pub const VARINT_TEST_CASES: [(&[u8], VarInt); 4] = [
            (&[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c], unsafe {
                VarInt::from_u64_unchecked(151_288_809_941_952_652)
            }),
            (&[0x9d, 0x7f, 0x3e, 0x7d], VarInt::from_u32(494_878_333)),
            (&[0x7b, 0xbd], VarInt::from_u32(15_293)),
            (&[0x25], VarInt::from_u32(37)),
        ];

        #[cfg(feature = "async")]
        #[cfg_attr(docsrs, doc(cfg(feature = "async")))]
        pub mod r#async {
            use super::*;
            use std::pin::Pin;
            use std::task::Context;
            use std::task::Poll;

            pub struct StepReader {
                data: Box<[u8]>,
                offset: usize,
                to_pending: bool,
            }

            impl StepReader {
                pub fn new<T>(data: T) -> Self
                where
                    T: Into<Box<[u8]>>,
                {
                    Self {
                        data: data.into(),
                        offset: 0,
                        to_pending: true,
                    }
                }
            }

            impl AsyncRead for StepReader {
                fn poll_read(
                    mut self: Pin<&mut Self>,
                    cx: &mut Context<'_>,
                    buf: &mut [u8],
                ) -> Poll<std::io::Result<usize>> {
                    let new_pending = !self.to_pending;
                    let to_pending = std::mem::replace(&mut self.to_pending, new_pending);

                    if buf.is_empty() {
                        return Poll::Ready(Ok(0));
                    }

                    if to_pending {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    } else if let Some(&byte) = self.data.get(self.offset) {
                        buf[0] = byte;
                        self.offset += 1;
                        Poll::Ready(Ok(1))
                    } else {
                        Poll::Ready(Ok(0))
                    }
                }
            }

            pub struct StepWriter {
                buffer: Vec<u8>,
                max_len: Option<usize>,
                to_pending: bool,
            }

            impl StepWriter {
                pub fn new(max_len: Option<usize>) -> Self {
                    Self {
                        buffer: Vec::new(),
                        max_len,
                        to_pending: true,
                    }
                }

                pub fn written(&self) -> &[u8] {
                    &self.buffer
                }
            }

            impl AsyncWrite for StepWriter {
                fn poll_write(
                    mut self: Pin<&mut Self>,
                    cx: &mut Context<'_>,
                    buf: &[u8],
                ) -> Poll<Result<usize, std::io::Error>> {
                    let new_pending = !self.to_pending;
                    let to_pending = std::mem::replace(&mut self.to_pending, new_pending);

                    if buf.is_empty() {
                        return Poll::Ready(Ok(0));
                    }

                    if to_pending {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    } else if self.buffer.len() < self.max_len.unwrap_or(usize::MAX) {
                        let byte = buf[0];
                        self.buffer.push(byte);
                        Poll::Ready(Ok(1))
                    } else {
                        Poll::Ready(Err(std::io::Error::new(
                            std::io::ErrorKind::ConnectionReset,
                            "Reached max len",
                        )))
                    }
                }
            }
        }

        #[cfg(feature = "async")]
        pub use r#async::StepReader;

        #[cfg(feature = "async")]
        pub use r#async::StepWriter;
    }
}
