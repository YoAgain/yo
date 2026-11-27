/// The continent of the proxy's data centre (spec 4.1 Round trip).
pub(crate) fn continent(cf_ray: &str) -> usize {
    let Some((_, colo)) = cf_ray.rsplit_once('-') else { return 3 };
    match colo {
        "AMS" | "ARN" | "ATH" | "BCN" | "BEG" | "BER" | "BLQ" | "BOD" | "BRU" | "BTS" | "BUD" | "CDG" | "CPH"
        | "DME" | "DUB" | "DUS" | "EDI" | "FCO" | "FRA" | "GVA" | "HAM" | "HEL" | "IST" | "KBP" | "KEF" | "KIV"
        | "LED" | "LHR" | "LIS" | "LJU" | "LUX" | "LYS" | "MAD" | "MAN" | "MRS" | "MUC" | "MXP" | "NAP" | "OSL"
        | "OTP" | "PMO" | "PRG" | "RIX" | "SKG" | "SOF" | "SOU" | "STR" | "SVQ" | "TLL" | "TXL" | "VIE" | "VNO"
        | "WAW" | "ZAG" | "ZRH" => 0,
        "ABQ" | "ANC" | "ASU" | "ATL" | "AUS" | "BGR" | "BNA" | "BOG" | "BOS" | "BSB" | "BUF" | "CLT" | "CMH"
        | "COR" | "CUR" | "CWB" | "DEN" | "DFW" | "DTW" | "ELP" | "EWR" | "EZE" | "FOR" | "FSD" | "GDL" | "GIG"
        | "GRU" | "GUA" | "GYE" | "GYN" | "HNL" | "IAD" | "IAH" | "IND" | "JAX" | "JFK" | "LAS" | "LAX" | "LIM"
        | "MCI" | "MCO" | "MDE" | "MEM" | "MEX" | "MIA" | "MSP" | "OMA" | "ORD" | "PDX" | "PHL" | "PHX" | "PIT"
        | "POA" | "PTY" | "QRO" | "REC" | "RIC" | "SAL" | "SAN" | "SCL" | "SEA" | "SJC" | "SJO" | "SJU" | "SLC"
        | "SMF" | "STL" | "TPA" | "UIO" | "YEG" | "YHZ" | "YOW" | "YUL" | "YVR" | "YWG" | "YXE" | "YYC" | "YYZ" => 1,
        "ALA" | "AMD" | "AMM" | "AUH" | "BAH" | "BEY" | "BKK" | "BLR" | "BOM" | "CAN" | "CCU" | "CEB" | "CGK"
        | "CGO" | "CMB" | "CTU" | "DAC" | "DAD" | "DEL" | "DOH" | "DXB" | "FOC" | "FUK" | "HAN" | "HGH" | "HKG"
        | "HND" | "HYD" | "ICN" | "ISB" | "JED" | "KBL" | "KHH" | "KHI" | "KIX" | "KMG" | "KTM" | "KUL" | "KWI"
        | "LHE" | "MAA" | "MCT" | "MNL" | "NKG" | "NRT" | "OKA" | "PEK" | "PNH" | "PVG" | "RGN" | "RUH" | "SGN"
        | "SIN" | "SZX" | "TAO" | "TAS" | "TBS" | "TLV" | "TPE" | "UBN" | "WUH" | "XIY" => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn continents_by_ray_suffix() {
        for (ray, continent) in [
            ("abc-FRA", 0),
            ("abc-GRU", 1),
            ("abc-extra-NRT", 2),
            ("abc-DXB", 2),
            ("abc-SYD", 3),
            ("abc-JNB", 3),
            ("abc-XYZ", 3),
            ("FRA", 3),
            ("abc-", 3),
        ] {
            assert_eq!(super::continent(ray), continent);
        }
    }
}
