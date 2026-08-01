//! Shared HTTP client construction for TTS backends.

/// Build a reqwest client suitable for the current platform.
///
/// On Android the system trust store is not available to rustls, so the Mozilla
/// root certificates are bundled via `webpki-root-certs`. The client also binds
/// to the IPv4 unspecified address so reqwest prefers IPv4 when resolving
/// dual-stack hosts: some Android networks return unusable IPv6 records and
/// reqwest can fail to fall back to IPv4, surfacing as "error sending request".
pub(crate) fn tls_client() -> reqwest::Client {
    #[cfg(target_os = "android")]
    {
        let certs: Vec<reqwest::Certificate> = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .filter_map(|c| reqwest::Certificate::from_der(c.as_ref()).ok())
            .collect();
        assert!(
            !certs.is_empty(),
            "no bundled root certificates were loaded; HTTPS cannot be used"
        );
        reqwest::Client::builder()
            .use_rustls_tls()
            .tls_certs_only(certs)
            .local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
            .build()
            .expect("failed to build HTTP client")
    }
    #[cfg(not(target_os = "android"))]
    {
        reqwest::Client::new()
    }
}
