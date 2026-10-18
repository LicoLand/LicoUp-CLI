use super::*;

pub(crate) fn read_stdio_rpc_line(
    reader: &mut impl BufRead,
    max_bytes: usize,
) -> io::Result<StdioRpcLine> {
    let mut line = Vec::new();
    let mut saw_bytes = false;
    let mut too_large = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if !saw_bytes {
                return Ok(StdioRpcLine::Eof);
            }
            break;
        }
        saw_bytes = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        if !too_large {
            if line.len().saturating_add(consumed) > max_bytes {
                too_large = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..consumed]);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    if too_large {
        return Ok(StdioRpcLine::TooLarge);
    }
    while line
        .last()
        .is_some_and(|byte| matches!(*byte, b'\n' | b'\r'))
    {
        line.pop();
    }
    Ok(StdioRpcLine::Request(line))
}
