//! Just enough OSC 1.0 to speak the link: messages with float, int and
//! string arguments, gathered into a bundle.
//!
//! An OSC message is its address, its type tags (`,fii`) and its arguments,
//! each string padded with nulls to a multiple of four bytes and each
//! number big-endian. A bundle is `#bundle`, a timetag, and its messages,
//! each after its length. Seoul reads a bundle's contents as they arrive,
//! so the timetag is OSC's "immediately".

/// One argument of a message.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Float(f32),
    Int(i32),
    Str(String),
}

/// OSC's timetag for "immediately".
const IMMEDIATELY: u64 = 1;

/// Writes bundles into a buffer that's kept between them.
#[derive(Default)]
pub struct BundleWriter {
    bytes: Vec<u8>,
    /// Where the message being written began, after its length.
    message_start: usize,
}

impl BundleWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a new bundle, dropping the last.
    pub fn begin(&mut self) {
        self.bytes.clear();
        pad_str(&mut self.bytes, "#bundle");
        self.bytes.extend_from_slice(&IMMEDIATELY.to_be_bytes());
    }

    /// Adds a message to the bundle.
    pub fn message(&mut self, address: &str, args: &[Arg]) {
        // Its length goes first, filled in once it's written
        let length_at = self.bytes.len();
        self.bytes.extend_from_slice(&[0; 4]);
        self.message_start = self.bytes.len();
        pad_str(&mut self.bytes, address);
        let tags_at = self.bytes.len();
        self.bytes.push(b',');
        for arg in args {
            self.bytes.push(match arg {
                Arg::Float(_) => b'f',
                Arg::Int(_) => b'i',
                Arg::Str(_) => b's',
            });
        }
        pad_to_four(&mut self.bytes, tags_at);
        for arg in args {
            match arg {
                Arg::Float(value) => self.bytes.extend_from_slice(&value.to_be_bytes()),
                Arg::Int(value) => self.bytes.extend_from_slice(&value.to_be_bytes()),
                Arg::Str(text) => pad_str(&mut self.bytes, text),
            }
        }
        let length = (self.bytes.len() - self.message_start) as u32;
        self.bytes[length_at..length_at + 4].copy_from_slice(&length.to_be_bytes());
    }

    /// The bundle so far.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Writes `text` and a null, padded to a multiple of four bytes.
fn pad_str(bytes: &mut Vec<u8>, text: &str) {
    let start = bytes.len();
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
    pad_to_four(bytes, start);
}

/// Pads with nulls until what's been written since `start` is a multiple
/// of four bytes.
fn pad_to_four(bytes: &mut Vec<u8>, start: usize) {
    while (bytes.len() - start) % 4 != 0 {
        bytes.push(0);
    }
}

/// Reads a bundle (or a bare message) back into its messages: what a
/// listener would see. `None` if it isn't OSC this module writes.
pub fn decode(bytes: &[u8]) -> Option<Vec<(String, Vec<Arg>)>> {
    let mut reader = Reader { bytes, at: 0 };
    if bytes.starts_with(b"#bundle\0") {
        reader.at = 16;
        let mut messages = Vec::new();
        while reader.at < bytes.len() {
            let length = reader.u32()? as usize;
            let element = bytes.get(reader.at..reader.at + length)?;
            messages.extend(decode(element)?);
            reader.at += length;
        }
        Some(messages)
    } else {
        let address = reader.str()?;
        let tags = reader.str()?;
        let mut args = Vec::new();
        for tag in tags.strip_prefix(',')?.chars() {
            args.push(match tag {
                'f' => Arg::Float(f32::from_bits(reader.u32()?)),
                'i' => Arg::Int(reader.u32()? as i32),
                's' => Arg::Str(reader.str()?),
                _ => return None,
            });
        }
        Some(vec![(address, args)])
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u32(&mut self) -> Option<u32> {
        let word = self.bytes.get(self.at..self.at + 4)?;
        self.at += 4;
        Some(u32::from_be_bytes(word.try_into().ok()?))
    }

    fn str(&mut self) -> Option<String> {
        let rest = self.bytes.get(self.at..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        let text = std::str::from_utf8(&rest[..end]).ok()?.to_string();
        self.at += (end + 1).div_ceil(4) * 4;
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_laid_out_as_osc_says() {
        let mut writer = BundleWriter::new();
        writer.begin();
        writer.message("/soba/cv/1", &[Arg::Float(0.5)]);
        let bytes = writer.bytes();
        assert_eq!(&bytes[..8], b"#bundle\0");
        assert_eq!(&bytes[8..16], &1u64.to_be_bytes());
        // "/soba/cv/1" + null is 11 bytes, padded to 12; ",f" + null to 4; one float
        assert_eq!(&bytes[16..20], &20u32.to_be_bytes());
        assert_eq!(&bytes[20..32], b"/soba/cv/1\0\0");
        assert_eq!(&bytes[32..36], b",f\0\0");
        assert_eq!(&bytes[36..40], &0.5f32.to_be_bytes());
        assert_eq!(bytes.len(), 40);
    }

    #[test]
    fn a_four_byte_address_still_gets_its_null() {
        let mut writer = BundleWriter::new();
        writer.begin();
        writer.message("/abc", &[]);
        // "/abc" + null pads to 8; "," + null pads to 4
        assert_eq!(&writer.bytes()[20..28], b"/abc\0\0\0\0");
        assert_eq!(&writer.bytes()[28..32], b",\0\0\0");
    }

    #[test]
    fn bundles_read_back() {
        let mut writer = BundleWriter::new();
        writer.begin();
        writer.message("/soba/clock", &[Arg::Float(120.0), Arg::Float(0.25), Arg::Int(9), Arg::Int(2)]);
        writer.message("/soba/gate/3", &[Arg::Int(1)]);
        writer.message("/soba/hello", &[Arg::Str("0.7.0".into())]);
        let messages = decode(writer.bytes()).unwrap();
        assert_eq!(
            messages,
            vec![
                ("/soba/clock".to_string(), vec![Arg::Float(120.0), Arg::Float(0.25), Arg::Int(9), Arg::Int(2)]),
                ("/soba/gate/3".to_string(), vec![Arg::Int(1)]),
                ("/soba/hello".to_string(), vec![Arg::Str("0.7.0".into())]),
            ]
        );
        // A new bundle starts afresh
        writer.begin();
        writer.message("/soba/transport", &[Arg::Int(0)]);
        assert_eq!(decode(writer.bytes()).unwrap().len(), 1);
    }
}
