//! InnerTube client definitions.
//!
//! Ported from upstream `YouTubeClient.kt`. YouTube gates stream delivery by
//! client, so the app keeps a set of known-good client identities and walks
//! them in priority order until one returns playable formats. The
//! `ANDROID_VR` and `IOS` clients are preferred because they still return
//! plain, unciphered `url` fields.

/// A YouTube InnerTube client identity.
#[derive(Debug, Clone, Copy)]
pub struct YtClient {
    pub name: &'static str,
    pub version: &'static str,
    /// `X-YouTube-Client-Name` header value (numeric client id as a string).
    pub id: &'static str,
    pub user_agent: &'static str,
    pub os_name: Option<&'static str>,
    pub os_version: Option<&'static str>,
    pub device_make: Option<&'static str>,
    pub device_model: Option<&'static str>,
    pub android_sdk_version: Option<&'static str>,
    pub build_id: Option<&'static str>,
    pub cronet_version: Option<&'static str>,
    pub package_name: Option<&'static str>,
    pub friendly_name: &'static str,
    /// Whether the client can carry an authenticated session.
    pub login_supported: bool,
    /// Whether `signatureTimestamp` must be sent with the player request.
    pub use_signature_timestamp: bool,
    /// Embedded clients embed the player in a third-party page.
    pub is_embedded: bool,
    /// Whether a web PO token should be attached to deciphered URLs.
    pub use_web_po_tokens: bool,
}

pub const ORIGIN_YOUTUBE_MUSIC: &str = "https://music.youtube.com";
pub const API_URL_YOUTUBE_MUSIC: &str = "https://music.youtube.com/youtubei/v1/";
pub const ORIGIN_YOUTUBE: &str = "https://www.youtube.com";
pub const API_URL_YOUTUBE: &str = "https://www.youtube.com/youtubei/v1/";

const UA_WEB: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0";

/// Desktop web music client. Used for search, browse, home, explore.
pub const WEB_REMIX: YtClient = YtClient {
    name: "WEB_REMIX",
    version: "1.20260213.01.00",
    id: "67",
    user_agent: UA_WEB,
    os_name: None,
    os_version: None,
    device_make: None,
    device_model: None,
    android_sdk_version: None,
    build_id: None,
    cronet_version: None,
    package_name: None,
    friendly_name: "Web Remix",
    login_supported: true,
    use_signature_timestamp: true,
    is_embedded: false,
    use_web_po_tokens: true,
};

/// Current ANDROID_VR pin — returns 100% direct-url formats and needs only
/// `visitorData`. Preferred first entry of the stream chain.
pub const ANDROID_VR_1_65_10: YtClient = YtClient {
    name: "ANDROID_VR",
    version: "1.65.10",
    id: "28",
    user_agent: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
    os_name: Some("Android"),
    os_version: Some("12L"),
    device_make: Some("Oculus"),
    device_model: Some("Quest 3"),
    android_sdk_version: Some("32"),
    build_id: None,
    cronet_version: None,
    package_name: None,
    friendly_name: "Android VR 1.65",
    login_supported: false,
    use_signature_timestamp: false,
    is_embedded: false,
    use_web_po_tokens: false,
};

/// Older ANDROID_VR pin, kept as a fallback.
pub const ANDROID_VR_1_43_32: YtClient = YtClient {
    name: "ANDROID_VR",
    version: "1.43.32",
    id: "28",
    user_agent: "com.google.android.apps.youtube.vr.oculus/1.43.32 (Linux; U; Android 12; en_US; Quest 3; Build/SQ3A.220605.009.A1; Cronet/107.0.5284.2)",
    os_name: Some("Android"),
    os_version: Some("12"),
    device_make: Some("Oculus"),
    device_model: Some("Quest 3"),
    android_sdk_version: Some("32"),
    build_id: Some("SQ3A.220605.009.A1"),
    cronet_version: Some("107.0.5284.2"),
    package_name: Some("com.google.android.apps.youtube.vr.oculus"),
    friendly_name: "Android VR 1.43",
    login_supported: false,
    use_signature_timestamp: false,
    is_embedded: false,
    use_web_po_tokens: false,
};

/// iOS client — direct URLs, good quality audio.
pub const IOS: YtClient = YtClient {
    name: "IOS",
    version: "21.03.1",
    id: "5",
    user_agent: "com.google.ios.youtube/21.03.1 (iPhone16,2; U; CPU iOS 18_2 like Mac OS X;)",
    os_name: Some("iOS"),
    os_version: Some("18.2.22C152"),
    device_make: Some("Apple"),
    device_model: Some("iPhone16,2"),
    android_sdk_version: None,
    build_id: None,
    cronet_version: None,
    package_name: Some("com.google.ios.youtube"),
    friendly_name: "iOS",
    login_supported: false,
    use_signature_timestamp: false,
    is_embedded: false,
    use_web_po_tokens: false,
};

/// Embedded TV player — bypasses age restriction without login.
pub const TVHTML5_SIMPLY_EMBEDDED_PLAYER: YtClient = YtClient {
    name: "TVHTML5_SIMPLY_EMBEDDED_PLAYER",
    version: "2.0",
    id: "85",
    user_agent: "Mozilla/5.0 (PlayStation; PlayStation 4/12.02) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.4 Safari/605.1.15",
    os_name: None,
    os_version: None,
    device_make: None,
    device_model: None,
    android_sdk_version: None,
    build_id: None,
    cronet_version: None,
    package_name: None,
    friendly_name: "TV Embedded",
    login_supported: true,
    use_signature_timestamp: true,
    is_embedded: true,
    use_web_po_tokens: false,
};

/// Android client with a signature timestamp.
pub const ANDROID: YtClient = YtClient {
    name: "ANDROID",
    version: "21.03.38",
    id: "3",
    user_agent: "com.google.android.youtube/21.03.38 (Linux; U; Android 14) gzip",
    os_name: Some("Android"),
    os_version: Some("14"),
    device_make: Some("Google"),
    device_model: Some("Pixel 7"),
    android_sdk_version: Some("34"),
    build_id: None,
    cronet_version: None,
    package_name: Some("com.google.android.youtube"),
    friendly_name: "Android",
    login_supported: true,
    use_signature_timestamp: true,
    is_embedded: false,
    use_web_po_tokens: false,
};

/// Web Creator client — authenticated, can serve age-restricted content.
pub const WEB_CREATOR: YtClient = YtClient {
    name: "WEB_CREATOR",
    version: "1.20260213.00.00",
    id: "62",
    user_agent: UA_WEB,
    os_name: None,
    os_version: None,
    device_make: None,
    device_model: None,
    android_sdk_version: None,
    build_id: None,
    cronet_version: None,
    package_name: None,
    friendly_name: "Web Creator",
    login_supported: true,
    use_signature_timestamp: true,
    is_embedded: false,
    use_web_po_tokens: true,
};

/// The ordered chain used when resolving a playable stream.
pub const STREAM_CHAIN: &[YtClient] = &[
    ANDROID_VR_1_65_10,
    IOS,
    TVHTML5_SIMPLY_EMBEDDED_PLAYER,
    ANDROID_VR_1_43_32,
    WEB_REMIX,
    ANDROID,
];

/// Clients that require a `visitorData` value to answer `OK`.
pub fn requires_visitor_data(client: &YtClient) -> bool {
    client.name == "ANDROID_VR" || client.name == "WEB_REMIX" || client.name == "WEB_CREATOR"
}

/// The long-standing public InnerTube key for a client family.
///
/// Keys rotate occasionally; the transport transparently retries without the
/// key when YouTube rejects one, so a stale value never breaks playback.
pub fn api_key(client: &YtClient) -> Option<&'static str> {
    match client.name {
        "WEB" | "WEB_REMIX" | "WEB_CREATOR" | "TVHTML5" => {
            Some("AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8")
        }
        "ANDROID" | "ANDROID_VR" | "ANDROID_CREATOR" => {
            Some("AIzaSyA8eiZmM1FaDVjRy-df2KTyQ_vz_yYM39w")
        }
        "IOS" | "VISIONOS" => Some("AIzaSyB-63vPrdThhKuerbB2N_l7Kwwcxj6yUAc"),
        _ => None,
    }
}
