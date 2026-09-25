//! Linux additionally proves that opening a FIFO never waits for a writer.

pub(super) fn create(path: &std::path::Path) {
    rustix::fs::mknodat(
        rustix::fs::CWD,
        path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
}
