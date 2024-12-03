use std::io::{ErrorKind, Read, Result, Write};

use byteorder::{NativeEndian, ReadBytesExt, WriteBytesExt};
use os_pipe::{pipe, PipeReader, PipeWriter};
use serde::{Deserialize, Serialize};

pub struct BidirPipe {
    rx: PipeReader,
    tx: PipeWriter,
}

impl Read for BidirPipe {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.rx.read(buf)
    }
}

impl Write for BidirPipe {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.tx.write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.tx.flush()
    }
}

pub fn bidir_pipe() -> (BidirPipe, BidirPipe) {
    let (rx1, tx1) = pipe().unwrap();
    let (rx2, tx2) = pipe().unwrap();

    (
        BidirPipe { rx: rx1, tx: tx2 },
        BidirPipe { rx: rx2, tx: tx1 },
    )
}

fn is_eof(err: &anyhow::Error) -> bool {
    if let Some(err) = err.downcast_ref::<std::io::Error>() {
        return err.kind() == ErrorKind::UnexpectedEof;
    }
    false
}

pub trait EofOk {
    fn eof_ok(self) -> Self;
}

impl EofOk for anyhow::Result<()> {
    fn eof_ok(self) -> Self {
        self.or_else(|err| if is_eof(&err) { Ok(()) } else { Err(err) })
    }
}

pub trait WriteSerde: WriteBytesExt {
    fn write_serde(&mut self, data: &impl Serialize) -> anyhow::Result<()> {
        let serialized = rmp_serde::to_vec_named(data)?;
        self.write_u64::<NativeEndian>(serialized.len() as _)?;
        self.write_all(&serialized)?;
        Ok(())
    }
}

impl<W: WriteBytesExt> WriteSerde for W {}

pub trait ReadSerde: ReadBytesExt {
    fn read_serde<T: for<'a> Deserialize<'a>>(&mut self) -> anyhow::Result<T> {
        let len = self.read_u64::<NativeEndian>()? as usize;
        let mut buffer = vec![0; len];
        self.read_exact(&mut buffer)?;
        Ok(rmp_serde::from_slice(&buffer)?)
    }
}

impl<R: ReadBytesExt> ReadSerde for R {}
