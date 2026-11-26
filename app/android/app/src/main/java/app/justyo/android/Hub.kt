package app.justyo.android

/**
 * The hub this build talks to (BuildConfig.HUB: the store app the live hub, debug and the device QA build the dev hub)
 * and its public pins, the same as Hub.swift. No private key is shipped.
 */
object Hub {
    /** The hub's address: BuildConfig.HUB, except on Google's Test Lab robots (the Play pre-launch report), which go
     *  to the dev hub so their made-up accounts never reach the live one. Set once in YoApplication.onCreate. */
    var url: String = BuildConfig.HUB
        private set
    val host: String get() = java.net.URI(url).host

    fun init(context: android.content.Context) {
        val testLab = runCatching {
            android.provider.Settings.System.getString(context.contentResolver, "firebase.test.lab") == "true"
        }.getOrDefault(false)
        if (testLab) url = "https://dev.justyo.app"
        // Oblivious HTTP to the hub's pinned key (spec 4.1); a debug or QA build goes direct after
        // `adb shell settings put global yo_direct 1` (measurements against the old way).
        val direct = (BuildConfig.DEBUG || BuildConfig.APPLICATION_ID.endsWith(".soak")) && runCatching {
            android.provider.Settings.Global.getString(context.contentResolver, "yo_direct") == "1"
        }.getOrDefault(false)
        uniffi.yo.useHub(url, direct)
    }

    /** Keys the hub signs key-handing answers with (spec 4.1): the one in use and an offline spare. */
    private val keys = mapOf(
        "hub.justyo.app" to listOf("X7-jqvmZLam-Oh_4VKIB2ozPLUACspOZ2K09v5R5yyY", "NJRBNxJ12bZa1JK8fCLCGFTgM1oJ658mrwKYCdgbmHY"),
        "dev.justyo.app" to listOf("LPiW1TOvv_ricbn7bXmS_fNdVbgtEw_3o7OdelvEdVo", "nFQuDMd-2Ueoec6oYG-Ty7lHQELq1OHs4TVODmPmryg"),
    )

    /** Sender-certificate trust roots (spec 4.1): in use and an offline spare. */
    private val trustRoots = mapOf(
        "hub.justyo.app" to listOf("yj7lt_zG5IrePgCcIsK7gQoVqFDsmOBfUvO4NyK1GF0", "mJjpj2WXfUy95PRcuA4tl9b_gZxJzRZhyGFq9iVglHU"),
        "dev.justyo.app" to listOf("kKVskVCuVwESdvLeYWAEVAQmT9mdwvkQ2uhOlnxXO3Q", "Fj-t9EAOsTDPSHPT1Y39FdbcIvl2I7aEEdqlcRAQc0c"),
    )

    /** zkgroup public params (spec 3.4). */
    private val zkParams = mapOf(
        "hub.justyo.app" to "ALb95DB0Ug3_2sTqjQrBkwhU80zTvUUOepEXF_UIP94kpNslgM89eclAFgszuPloThXdfYihyreYIu_4EH7R8yoOGMKgWiIwxjUcjjuX-n664-nDWet72Epx7NRonkZpBtCTv5Wdy6KVNrRwyb6gS5K3kkOLza9T2wx-jUUri-Q4auRulgrLoJ5WTldL5x2HIXeLvv2lQHjEfvbKUKl3dm-A92r8aHYBP2nsoAykAJ5U4oWvsDRGTnLdtMy-CIM4O5TuZLI7NB6mLOUSsCJKU4cZgz0LsglhzS6V9Jsa64kZzoIsqOIqJm9a-zsRCo7zkWTDRK1C5M77AtPil6OdejogIsru0LXoD74GYO0OjY8XEMPrxcRdqrcVUb1mhFxIGwJxGkaEx9Z-a0q4Oti8mO11d0R1OrTBrM8R_SAuo_FnlKRAkyHrya3EsJGeUenYsNgeYkGV8Dw2QZdKTJZKQHPQHnl1HuT0XJU8QzsXMTFMymujijlIPgF3-5X0TKqPE86rDH2fzXcmfo2nsckJ560DRW8VA2jV3wcKPa3i_hFqJklg7iqMm9Zi-s0dkLxgY6GK69MspMf5imJ7eVYLzlIY2Dr9AxIDIytElgpXsugufEtCC4gBn2N2dpfC6qiQKOpZ7_TL3wmFiFayGsplxPz4alSfEPvfEceIU0x7ZzxlfPsBh48ne0GtldAyg4v8t0C2Pmz5qZSVqQSfbhu_AS3O2gVOeRf1E0wqBkfuz97PPbX0R_MpUD9FmAFC2DTnTkS4kU-ygDsFRDe8bRTTHoXNfn9Caby2jlp5ZQKSfipR_HmLO5mG8PwyxzP5EQ0PVcBIozgHanfhgSUku_cXiDFOxLZfbhALRp3yWAYnG5myHGnLJhzld8RJtIxsJ0QnAA",
        "dev.justyo.app" to "AABte-FyyNG7G0rm_Swi9dLOf6khh32C0-TmfHELCW5J4k4m_beuhn-LDZ6GFT19xzzza_-DQfEKUPdsKy99Knl4FgkgVTu8j8yZP5JoagLdul10PyCTaUcx649Z-JJuUWidD_T_ZYOgvSS61z9QixJQB6LAB_k0AYFY563qzWItEncY_vD68l2OUM1aTDNuXO6QeT0MgeRwrlsqLhF10C2SMQJotdkaEOGAMdr4E261qG0CNuVEua6ojEXs7BXCOpTIPvZhkxQf6N2-WWXVXyrm65kQSDkE32SYFSTxhP9dNNdVop_SDJE24rMYPJy1SH5DD1gs7Y67w0RNQIooUHmaUn3WlOjHXFMgwHpDDMGlDuMqgEMkz1D-ArhgAoYsXG6LduuWJgSRWi3KED6jZRm2QCUpd_LIkZKR2vmQp_Y68IvRf_myayJE33VuqTbfT0jp9KBeiJuIOdqubZBWb2vG_jI0ihgyFRJPTrFtpW0iv1tpngyN0m7_V3I-jUlzIGQSJRJDvDvlEvKPMDeA9Sc2Nh5tRY3JiMTqcFwCVMsbCujL_mhC4VBfXUFy-urGdLiqdTBZF53mInuT_yR8VFoSPLeTi7mHnRSlACAnh_GFnE80FlbXVEbW8EdMXzwycwLy3GajU4z0wOLmeI58jYlwZw8A-dNcB2CHG4zBwpkTiD2ZHZcdAL4kSljBUqE2Tml9cfKtGzqcvH7zrxOhjCL6Sn9IyV36TVf6lKf-oRjDHsPHP9pSp3apVZfG1dEdRQr2UEvHRHla3C8jQrte8e02zrqQwG0KxsUTueOVgiVsWtaUV3YohIHtxbSUOljwoaA-iRmPQsvUHgcpL2UoOghIMYvKSbwRIde_-NYhyQVSCBpcgz5bhRbHcc-uS_j7Qg",
    )

    val hubKeys: List<String> get() = keys.getValue(host)
    val roots: List<String> get() = trustRoots.getValue(host)
    val groupParams: String get() = zkParams.getValue(host)
}
