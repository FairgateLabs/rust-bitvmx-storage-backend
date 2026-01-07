use age::{
    scrypt::Identity,
    secrecy::SecretString,
    stream::{StreamReader, StreamWriter},
    Decryptor, Encryptor,
};
use std::io::{self, BufRead, Read, Write};

pub struct BackupFileWriter<W: Write> {
    inner: StreamWriter<W>,
}

impl<W: Write> BackupFileWriter<W> {
    pub fn new(writer: W, password: Vec<u8>) -> io::Result<Self> {
        let passphrase = SecretString::new(hex::encode(password).into());
        let encryptor = Encryptor::with_user_passphrase(passphrase);
        let stream_writer = encryptor.wrap_output(writer)?;
        Ok(BackupFileWriter {
            inner: stream_writer,
        })
    }

    pub fn finish(self) -> io::Result<W> {
        self.inner.finish()
    }
}

impl<W: Write> Write for BackupFileWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub struct BackupFileReader<R: Read> {
    inner: StreamReader<R>,
    buf: Vec<u8>,
    pos: usize,
    cap: usize,
}

impl<R: Read> BackupFileReader<R> {
    pub fn new(reader: R, password: Vec<u8>) -> io::Result<Self> {
        let passphrase = SecretString::new(hex::encode(password).into());
        let decryptor =
            Decryptor::new(reader).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let identities: Vec<Box<dyn age::Identity>> = vec![Box::new(Identity::new(passphrase))];
        let stream_reader = decryptor
            .decrypt(identities.iter().map(|i| i.as_ref()))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        Ok(BackupFileReader {
            inner: stream_reader,
            buf: vec![0; 8192],
            pos: 0,
            cap: 0,
        })
    }
}

impl<R: Read> BufRead for BackupFileReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.cap {
            let n = self.inner.read(&mut self.buf)?;
            self.cap = n;
            self.pos = 0;
        }
        Ok(&self.buf[self.pos..self.cap])
    }

    fn consume(&mut self, amt: usize) {
        self.pos = std::cmp::min(self.pos + amt, self.cap);
    }
}

impl<R: Read> Read for BackupFileReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(out.len());
        out[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }
}
