pub struct Writer<'w> {
    indent_step: usize,
    buf: &'w mut dyn std::io::Write,
}

impl std::fmt::Debug for Writer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer").field("indent_step", &self.indent_step).finish_non_exhaustive()
    }
}

impl std::io::Write for Writer<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> { self.buf.write(buf) }

    fn flush(&mut self) -> std::io::Result<()> { self.buf.flush() }
}
