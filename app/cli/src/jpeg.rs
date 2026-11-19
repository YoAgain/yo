//! A JPEG as a photo goes (spec 5): no metadata, at most 1600 px a side. The apps do this with the system's image
//! tools; a server has none.

/// A JPEG's width and height from its frame header, if it has one where it should.
pub fn size(bytes: &[u8]) -> Option<(u16, u16)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while bytes.get(i) == Some(&0xFF) {
        let marker = *bytes.get(i + 1)?;
        // A start of frame (not DHT, JPG, DAC): height, then width.
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            return Some((be16(i + 7)?, be16(i + 5)?));
        }
        i += 2 + be16(i + 2)? as usize;
    }
    None
}

/// The JPEG with its metadata segments (APP1 to APP15, comments: EXIF, GPS, XMP) taken out, and its width and height,
/// at most 1600 px; `None` for no JPEG or a larger one.
pub fn clean(bytes: &[u8]) -> Option<(Vec<u8>, u16, u16)> {
    let (width, height) = size(bytes)?;
    if width.max(height) > 1600 {
        return None;
    }
    let be16 = |i: usize| Some(u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
    let mut out = bytes[..2].to_vec();
    let mut i = 2;
    loop {
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(i + 1)?;
        if marker == 0xD9 {
            // The end of the image: nothing after it goes along.
            out.extend_from_slice(&[0xFF, 0xD9]);
            return Some((out, width, height));
        }
        let end = i + 2 + be16(i + 2)? as usize;
        let segment = bytes.get(i..end)?;
        if !((0xE1..=0xEF).contains(&marker) || marker == 0xFE) {
            out.extend_from_slice(segment);
        }
        i = end;
        if marker == 0xDA {
            // A scan's data up to the next marker (0xFF then neither 0 nor a restart): progressive JPEGs have several.
            let start = i;
            while i + 1 < bytes.len()
                && !(bytes[i] == 0xFF && bytes[i + 1] != 0 && !(0xD0..=0xD7).contains(&bytes[i + 1]))
            {
                i += 1;
            }
            out.extend_from_slice(bytes.get(start..i)?);
            if i + 1 >= bytes.len() {
                return None; // no end of image
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_goes_and_the_size_is_read() {
        // A frame header after an APP0 segment: 720 wide, 1280 high; then the scan.
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 17, 8, 0x05, 0x00, 0x02, 0xD0, 3, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0xFF, 0xDA, 0, 2, 1, 2, 3, 0xFF, 0xD9,
        ];
        assert_eq!(size(&jpeg), Some((720, 1280)));
        assert_eq!(size(&jpeg[..12]), None);
        assert_eq!(size(b"not a jpeg"), None);
        // An APP1 "Exif" segment goes, the rest stays as it was.
        let mut with_exif = jpeg[..2].to_vec();
        with_exif.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x08, b'E', b'x', b'i', b'f', 0, 0]);
        with_exif.extend_from_slice(&jpeg[2..]);
        let (cleaned, w, h) = clean(&with_exif).unwrap();
        assert_eq!((cleaned.as_slice(), w, h), (&jpeg[..], 720, 1280));
        // A comment after the first scan, a second scan, and bytes after the end: the comment and the tail go.
        let end = jpeg.len() - 2;
        let mut progressive = jpeg[..end].to_vec();
        progressive.extend_from_slice(&[0xFF, 0xFE, 0, 4, b'h', b'i', 0xFF, 0xDA, 0, 2, 9, 0xFF, 0x00, 9, 0xFF, 0xD9]);
        progressive.extend_from_slice(b"trailing");
        let mut expected = jpeg[..end].to_vec();
        expected.extend_from_slice(&[0xFF, 0xDA, 0, 2, 9, 0xFF, 0x00, 9, 0xFF, 0xD9]);
        assert_eq!(clean(&progressive).unwrap().0, expected);
        // Larger than 1600 px: none.
        let mut large = jpeg;
        large[13..15].copy_from_slice(&1601u16.to_be_bytes());
        assert_eq!(clean(&large), None);
    }
}
