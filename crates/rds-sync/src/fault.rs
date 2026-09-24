//! Test-only process/failure injection at durable transaction boundaries.
//! Production builds contain no environment switch or mutable fault state.

use std::fs::File;
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Point {
    Created,
    PartialWrite,
    Written,
    FileSynced,
    Renamed,
    DestinationSynced,
    SourceSynced,
    PartRemoved,
    MetaRemoved,
    PartsRemoved,
    JournalRemoved,
}

#[cfg(not(test))]
#[inline]
pub(crate) fn hit(_point: Point) -> io::Result<()> {
    Ok(())
}

#[cfg(not(test))]
pub(crate) fn write(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    // Keep the production write unsplit; only test builds inject a torn body.
    hit(Point::PartialWrite)?;
    file.write_all(bytes)
}

#[cfg(test)]
pub(crate) fn write(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    let (first, second) = bytes.split_at(bytes.len() / 2);
    file.write_all(first)?;
    hit(Point::PartialWrite)?;
    file.write_all(second)
}

#[cfg(test)]
thread_local! {
    static FAULT: std::cell::Cell<Option<(Point, Action)>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum Action {
    Error(rustix::io::Errno),
    Exit,
}

#[cfg(test)]
pub(crate) fn hit(point: Point) -> io::Result<()> {
    FAULT.with(|fault| {
        if let Some((at, action)) = fault.get()
            && at == point
        {
            fault.set(None);
            match action {
                Action::Error(errno) => return Err(errno.into()),
                Action::Exit => std::process::exit(86),
            }
        }
        Ok(())
    })
}

#[cfg(test)]
pub(crate) struct Armed;

#[cfg(test)]
impl Armed {
    pub(crate) fn assert_fired(&self) {
        FAULT.with(|fault| assert!(fault.get().is_none(), "fault point was not reached"));
    }
}

#[cfg(test)]
pub(crate) fn arm(point: Point, action: Action) -> Armed {
    FAULT.with(|fault| assert!(fault.replace(Some((point, action))).is_none()));
    Armed
}

#[cfg(test)]
impl Drop for Armed {
    fn drop(&mut self) {
        FAULT.with(|fault| fault.set(None));
    }
}
