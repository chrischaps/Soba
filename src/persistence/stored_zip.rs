//! A zip archive written by hand, its files stored as they are.
//!
//! The browser hands a patch over whole as one download: the patch beside
//! the takes it plays (see `tab_files`). Stored, not deflated, keeps the
//! format small (a header before each file, a directory at the end, and a
//! CRC-32 of each), and WAVs barely compress anyway. Any unzipper opens it.

/// A zip archive being written, file by file.
pub struct StoredZip {
    bytes: Vec<u8>,
    directory: Vec<u8>,
    count: u16,
    /// When the files were last changed, as MS-DOS stores it: (time, date).
    modified: (u16, u16),
}

impl StoredZip {
    /// An empty archive whose files read as changed at the date and time
    /// given (local, to the second; zip has no time zone).
    pub fn new(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Self {
        let time = (hour as u16) << 11 | (minute as u16) << 5 | ((second as u16) / 2);
        let date = year.saturating_sub(1980).min(127) << 9 | (month as u16) << 5 | day as u16;
        Self { bytes: Vec::new(), directory: Vec::new(), count: 0, modified: (time, date) }
    }

    /// Adds `contents` as the file `path`, with forward slashes
    /// (`Song samples/Take 1.wav`). Fails past what a zip without its
    /// 64-bit extension holds: 4 GB, or 65,535 files.
    pub fn add(&mut self, path: &str, contents: &[u8]) -> Result<(), String> {
        let offset = u32::try_from(self.bytes.len()).map_err(|_| "it's over 4 GB".to_string())?;
        let size = u32::try_from(contents.len()).map_err(|_| format!("{path} is over 4 GB"))?;
        self.count = self.count.checked_add(1).ok_or("it holds too many files")?;
        let name = path.as_bytes();
        let crc = crc32(contents);
        let (time, date) = self.modified;

        // What the header before the file and its entry in the directory share
        let mut shared = Vec::with_capacity(26);
        shared.extend(20u16.to_le_bytes()); // version needed: 2.0
        shared.extend(0x0800u16.to_le_bytes()); // flags: the name is UTF-8
        shared.extend(0u16.to_le_bytes()); // stored
        shared.extend(time.to_le_bytes());
        shared.extend(date.to_le_bytes());
        shared.extend(crc.to_le_bytes());
        shared.extend(size.to_le_bytes()); // compressed
        shared.extend(size.to_le_bytes()); // and not
        shared.extend((name.len() as u16).to_le_bytes());
        shared.extend(0u16.to_le_bytes()); // no extra field

        self.bytes.extend(0x0403_4b50u32.to_le_bytes());
        self.bytes.extend(&shared);
        self.bytes.extend(name);
        self.bytes.extend(contents);

        self.directory.extend(0x0201_4b50u32.to_le_bytes());
        self.directory.extend(20u16.to_le_bytes()); // made by: 2.0, MS-DOS
        self.directory.extend(&shared);
        self.directory.extend(0u16.to_le_bytes()); // no comment
        self.directory.extend(0u16.to_le_bytes()); // disk 0
        self.directory.extend(0u16.to_le_bytes()); // internal attributes
        self.directory.extend(0u32.to_le_bytes()); // external attributes
        self.directory.extend(offset.to_le_bytes());
        self.directory.extend(name);
        Ok(())
    }

    /// The archive's bytes: the files, then the directory of them.
    pub fn finish(mut self) -> Result<Vec<u8>, String> {
        let too_big = || "it's over 4 GB".to_string();
        let start = u32::try_from(self.bytes.len()).map_err(|_| too_big())?;
        let length = u32::try_from(self.directory.len()).map_err(|_| too_big())?;
        self.bytes.append(&mut self.directory);
        self.bytes.extend(0x0605_4b50u32.to_le_bytes());
        self.bytes.extend(0u16.to_le_bytes()); // this disk
        self.bytes.extend(0u16.to_le_bytes()); // the directory's disk
        self.bytes.extend(self.count.to_le_bytes()); // entries on this disk
        self.bytes.extend(self.count.to_le_bytes()); // and in all
        self.bytes.extend(length.to_le_bytes());
        self.bytes.extend(start.to_le_bytes());
        self.bytes.extend(0u16.to_le_bytes()); // no comment
        Ok(self.bytes)
    }
}

/// The CRC-32 zip checks each file by (the IEEE polynomial, reflected).
pub fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut n = 0;
        while n < 256 {
            let mut c = n as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            table[n] = c;
            n += 1;
        }
        table
    };
    !bytes.iter().fold(!0u32, |crc, &b| TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn crc32_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn an_unzipper_reads_back_every_file() {
        let wav: Vec<u8> = (0..10_000u32).map(|n| (n * 7) as u8).collect();
        let mut zip = StoredZip::new(2026, 10, 15, 14, 30, 58);
        zip.add("Song.json", b"{\"name\":\"Song\"}").unwrap();
        zip.add("Song samples/Take 1 \u{2013} ah.wav", &wav).unwrap();
        let bytes = zip.finish().unwrap();

        let mut archive = ::zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 2);
        let mut text = String::new();
        archive.by_name("Song.json").unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "{\"name\":\"Song\"}");

        let mut take = archive.by_name("Song samples/Take 1 \u{2013} ah.wav").unwrap();
        let modified = take.last_modified().unwrap();
        assert_eq!((modified.year(), modified.month(), modified.day()), (2026, 10, 15));
        assert_eq!((modified.hour(), modified.minute(), modified.second()), (14, 30, 58));
        let mut read = Vec::new();
        take.read_to_end(&mut read).unwrap(); // checks its CRC as it reads
        assert_eq!(read, wav);
    }

    #[test]
    fn an_empty_archive_is_still_a_zip() {
        let bytes = StoredZip::new(2026, 1, 1, 0, 0, 0).finish().unwrap();
        assert_eq!(bytes.len(), 22);
        assert_eq!(::zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap().len(), 0);
    }
}
