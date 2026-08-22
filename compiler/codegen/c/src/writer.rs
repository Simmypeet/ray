use std::{
    fmt::Display,
    io::{self, Write},
};

const INDENT: &str = "    ";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnclosingPair {
    Parens,
    Braces,
    Brackets,
}

impl<'w> Writer<'w> {
    #[must_use]
    pub fn new(buf: &'w mut dyn std::io::Write) -> Self { Self { indent_step: 0, buf } }
}

impl Writer<'_> {
    fn write_indent(&mut self) -> io::Result<()> {
        for _ in 0..self.indent_step {
            self.write_all(INDENT.as_bytes())?;
        }

        Ok(())
    }

    pub(crate) async fn write_indent_line(
        &mut self,
        write_line: impl AsyncFnOnce(&mut Writer) -> io::Result<()>,
    ) -> io::Result<()> {
        self.write_indent()?;
        write_line(self).await?;
        writeln!(self)
    }

    pub(crate) async fn write_braced_block<A>(
        &mut self,
        write_inner: impl AsyncFnOnce(&mut Writer) -> io::Result<A>,
    ) -> io::Result<A> {
        writeln!(self, "{{")?;
        self.indent_step += 1;

        let result = write_inner(self).await;

        self.indent_step -= 1;
        let result = result?;

        self.write_indent()?;
        write!(self, "}}")?;

        Ok(result)
    }

    pub async fn write_enclosing_pair<A>(
        &mut self,
        delim_pair: EnclosingPair,
        write_inner: impl AsyncFnOnce(&mut Writer) -> std::io::Result<A>,
    ) -> std::io::Result<A> {
        let (open, close) = match delim_pair {
            EnclosingPair::Parens => ('(', ')'),
            EnclosingPair::Braces => ('{', '}'),
            EnclosingPair::Brackets => ('[', ']'),
        };

        write!(self, "{open}")?;
        let result = write_inner(self).await?;
        write!(self, "{close}")?;

        Ok(result)
    }

    pub async fn write_separated_list<A>(
        &mut self,
        delim_pair: EnclosingPair,
        items: impl IntoIterator<Item = A>,
        separator: impl Display,
        mut write_item: impl AsyncFnMut(&mut Writer, A) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.write_enclosing_pair(delim_pair, async move |writer| {
            let mut first = true;

            for item in items {
                if !first {
                    write!(writer, "{separator}")?;
                }

                write_item(writer, item).await?;

                first = false;
            }

            Ok(())
        })
        .await
    }
}
