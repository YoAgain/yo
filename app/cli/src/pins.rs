//! The public pins of the two hubs the apps know (as `Hub.kt` / `Hub.swift`): taken when the hub is one of them and
//! no flag or environment gives its own. No private key here.

/// The hub a fresh `yo` talks to: the live one, as the store apps.
pub const DEFAULT_HUB: &str = "https://hub.justyo.app";

pub struct Pins {
    pub host: &'static str,
    pub hub_keys: &'static [&'static str],
    pub trust_roots: &'static [&'static str],
    pub zk_params: &'static str,
}

pub const PINS: &[Pins] = &[
    Pins {
        host: "hub.justyo.app",
        hub_keys: &["X7-jqvmZLam-Oh_4VKIB2ozPLUACspOZ2K09v5R5yyY", "NJRBNxJ12bZa1JK8fCLCGFTgM1oJ658mrwKYCdgbmHY"],
        trust_roots: &["yj7lt_zG5IrePgCcIsK7gQoVqFDsmOBfUvO4NyK1GF0", "mJjpj2WXfUy95PRcuA4tl9b_gZxJzRZhyGFq9iVglHU"],
        zk_params: "ALb95DB0Ug3_2sTqjQrBkwhU80zTvUUOepEXF_UIP94kpNslgM89eclAFgszuPloThXdfYihyreYIu_4EH7R8yoOGMKgWiIwxjUcjjuX-n664-nDWet72Epx7NRonkZpBtCTv5Wdy6KVNrRwyb6gS5K3kkOLza9T2wx-jUUri-Q4auRulgrLoJ5WTldL5x2HIXeLvv2lQHjEfvbKUKl3dm-A92r8aHYBP2nsoAykAJ5U4oWvsDRGTnLdtMy-CIM4O5TuZLI7NB6mLOUSsCJKU4cZgz0LsglhzS6V9Jsa64kZzoIsqOIqJm9a-zsRCo7zkWTDRK1C5M77AtPil6OdejogIsru0LXoD74GYO0OjY8XEMPrxcRdqrcVUb1mhFxIGwJxGkaEx9Z-a0q4Oti8mO11d0R1OrTBrM8R_SAuo_FnlKRAkyHrya3EsJGeUenYsNgeYkGV8Dw2QZdKTJZKQHPQHnl1HuT0XJU8QzsXMTFMymujijlIPgF3-5X0TKqPE86rDH2fzXcmfo2nsckJ560DRW8VA2jV3wcKPa3i_hFqJklg7iqMm9Zi-s0dkLxgY6GK69MspMf5imJ7eVYLzlIY2Dr9AxIDIytElgpXsugufEtCC4gBn2N2dpfC6qiQKOpZ7_TL3wmFiFayGsplxPz4alSfEPvfEceIU0x7ZzxlfPsBh48ne0GtldAyg4v8t0C2Pmz5qZSVqQSfbhu_AS3O2gVOeRf1E0wqBkfuz97PPbX0R_MpUD9FmAFC2DTnTkS4kU-ygDsFRDe8bRTTHoXNfn9Caby2jlp5ZQKSfipR_HmLO5mG8PwyxzP5EQ0PVcBIozgHanfhgSUku_cXiDFOxLZfbhALRp3yWAYnG5myHGnLJhzld8RJtIxsJ0QnAA",
    },
    Pins {
        host: "dev.justyo.app",
        hub_keys: &["LPiW1TOvv_ricbn7bXmS_fNdVbgtEw_3o7OdelvEdVo", "nFQuDMd-2Ueoec6oYG-Ty7lHQELq1OHs4TVODmPmryg"],
        trust_roots: &["kKVskVCuVwESdvLeYWAEVAQmT9mdwvkQ2uhOlnxXO3Q", "Fj-t9EAOsTDPSHPT1Y39FdbcIvl2I7aEEdqlcRAQc0c"],
        zk_params: "AABte-FyyNG7G0rm_Swi9dLOf6khh32C0-TmfHELCW5J4k4m_beuhn-LDZ6GFT19xzzza_-DQfEKUPdsKy99Knl4FgkgVTu8j8yZP5JoagLdul10PyCTaUcx649Z-JJuUWidD_T_ZYOgvSS61z9QixJQB6LAB_k0AYFY563qzWItEncY_vD68l2OUM1aTDNuXO6QeT0MgeRwrlsqLhF10C2SMQJotdkaEOGAMdr4E261qG0CNuVEua6ojEXs7BXCOpTIPvZhkxQf6N2-WWXVXyrm65kQSDkE32SYFSTxhP9dNNdVop_SDJE24rMYPJy1SH5DD1gs7Y67w0RNQIooUHmaUn3WlOjHXFMgwHpDDMGlDuMqgEMkz1D-ArhgAoYsXG6LduuWJgSRWi3KED6jZRm2QCUpd_LIkZKR2vmQp_Y68IvRf_myayJE33VuqTbfT0jp9KBeiJuIOdqubZBWb2vG_jI0ihgyFRJPTrFtpW0iv1tpngyN0m7_V3I-jUlzIGQSJRJDvDvlEvKPMDeA9Sc2Nh5tRY3JiMTqcFwCVMsbCujL_mhC4VBfXUFy-urGdLiqdTBZF53mInuT_yR8VFoSPLeTi7mHnRSlACAnh_GFnE80FlbXVEbW8EdMXzwycwLy3GajU4z0wOLmeI58jYlwZw8A-dNcB2CHG4zBwpkTiD2ZHZcdAL4kSljBUqE2Tml9cfKtGzqcvH7zrxOhjCL6Sn9IyV36TVf6lKf-oRjDHsPHP9pSp3apVZfG1dEdRQr2UEvHRHla3C8jQrte8e02zrqQwG0KxsUTueOVgiVsWtaUV3YohIHtxbSUOljwoaA-iRmPQsvUHgcpL2UoOghIMYvKSbwRIde_-NYhyQVSCBpcgz5bhRbHcc-uS_j7Qg",
    },
];

/// The pins for a hub URL, if it is one of the known hubs.
pub fn for_hub(url: &str) -> Option<&'static Pins> {
    let host = url.trim_start_matches("https://").trim_start_matches("http://").split(['/', ':']).next()?;
    PINS.iter().find(|p| p.host == host)
}
