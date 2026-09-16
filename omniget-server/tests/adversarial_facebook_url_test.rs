//! Comprehensive Adversarial Challenge & Stress-Test Suite for Milestone 2
//! Target: `parse_facebook_url` in `omniget-server/src/tools/facebook_post.rs`
//!
//! Attack Vectors Tested:
//! 1. Extreme string lengths, trailing/leading whitespace, URL-encoded spaces.
//! 2. Hostile SSRF attempts, domain spoofing, userinfo abuse, non-HTTP schemes.
//! 3. Null bytes, control characters, path traversal, malformed payloads.
//! 4. Complex tracking query strings, multiple `#` fragments, fragment smuggling.
//! 5. Non-post Facebook paths (Marketplace, Messages, Friends, Notifications, Search, Login, Settings).
//! 6. Comprehensive valid format accuracy & ID extraction invariants.
//! 7. High-iteration pseudorandom fuzzing ensuring zero panics.

#[allow(dead_code)]
#[path = "../src/tools/facebook_post.rs"]
mod facebook_post;

use facebook_post::{is_tracking_param, parse_facebook_url, validate_post_id, FacebookExtractError};

// ============================================================================
// VECTOR 1: Extreme String Lengths, Whitespace, & URL-Encoded Characters
// ============================================================================

#[test]
fn test_vector1_extreme_length_post_id() {
    // 50,000 character alphanumeric post ID
    let massive_id: String = "9".repeat(50_000);
    let url = format!("https://www.facebook.com/posts/{}", massive_id);
    let res = parse_facebook_url(&url);
    assert!(res.is_ok(), "Massive alphanumeric post ID should parse without panic");
    let info = res.unwrap();
    assert_eq!(info.id, massive_id);
    assert_eq!(info.kind, "post");
}

#[test]
fn test_vector1_massive_query_string_padding() {
    // 100,000 character tracking query padding
    let massive_junk = "A".repeat(100_000);
    let url = format!(
        "https://www.facebook.com/zuck/posts/123456789?fbclid={}&mibextid={}",
        massive_junk, massive_junk
    );
    let res = parse_facebook_url(&url);
    assert!(res.is_ok(), "100KB query string should be stripped without stack overflow or panic");
    let info = res.unwrap();
    assert_eq!(info.id, "123456789");
    assert!(!info.canonical_url.contains("fbclid"));
    assert!(!info.canonical_url.contains("mibextid"));
}

#[test]
fn test_vector1_leading_trailing_whitespace_variations() {
    let variants = [
        "   https://www.facebook.com/posts/123456789   ",
        "\t\thttps://www.facebook.com/posts/123456789\t\n",
        "\r\n\r\nhttps://www.facebook.com/posts/123456789\r\n",
        "  \t \r\n https://www.facebook.com/posts/123456789 \n \t  ",
        "   123456789   ",
        "\t\tpfbid02AbCdEfGh123\n",
        "   https://fb.watch/k7A8b9C_d1/   ",
        " \t https://www.facebook.com/reel/987654321098765/ \n ",
    ];

    for raw in variants {
        let res = parse_facebook_url(raw);
        assert!(
            res.is_ok(),
            "Whitespace-padded URL or ID '{}' should parse cleanly, got error: {:?}",
            raw,
            res.err()
        );
        let info = res.unwrap();
        assert!(info.id == "123456789" || info.id == "pfbid02AbCdEfGh123" || info.id == "k7A8b9C_d1" || info.id == "987654321098765");
    }
}

#[test]
fn test_vector1_url_encoded_spaces_and_special_chars() {
    // URL-encoded space in post ID: %20 should fail validate_post_id because % is not alphanumeric
    let url_with_encoded_space_in_id = "https://www.facebook.com/posts/123%2045";
    let res = parse_facebook_url(url_with_encoded_space_in_id);
    assert!(
        matches!(res, Err(FacebookExtractError::InvalidInput(_))),
        "Post ID with %20 must be rejected with InvalidInput, got: {:?}",
        res
    );

    // URL-encoded slash in ID: %2F
    let url_with_encoded_slash = "https://www.facebook.com/posts/123%2F45";
    let res2 = parse_facebook_url(url_with_encoded_slash);
    assert!(
        matches!(res2, Err(FacebookExtractError::InvalidInput(_))),
        "Post ID with %2F must be rejected with InvalidInput, got: {:?}",
        res2
    );

    // Double encoded space: %2520
    let url_double_encoded = "https://www.facebook.com/posts/123%252045";
    let res3 = parse_facebook_url(url_double_encoded);
    assert!(
        matches!(res3, Err(FacebookExtractError::InvalidInput(_))),
        "Post ID with %2520 must be rejected with InvalidInput, got: {:?}",
        res3
    );

    // Raw ID with space: "123 456"
    let raw_space = parse_facebook_url("123 456");
    assert!(
        matches!(raw_space, Err(FacebookExtractError::InvalidInput(_))),
        "Raw post ID with space must be rejected, got: {:?}",
        raw_space
    );
}

// ============================================================================
// VECTOR 2: Hostile SSRF Attempts, Domain Spoofing, & Scheme Abuse
// ============================================================================

#[test]
fn test_vector2_hostile_ssrf_and_domain_spoofing() {
    let hostile_urls = [
        // Subdomain append attacks (attacker controls apex)
        "http://facebook.com.evil.com/posts/123456",
        "https://facebook.com.attacker.org/reel/123456",
        "https://fb.com.hacker.io/watch/?v=123456",
        "https://fb.watch.phishing.net/abc123",
        // Prefix lookalikes
        "https://evilfacebook.com/posts/123456",
        "https://notfacebook.com/posts/123456",
        "https://fakefb.com/posts/123456",
        "https://myfb.watch/abc123",
        "https://evil-facebook.com/posts/123456",
        // Internal networks and localhost SSRF
        "http://127.0.0.1/posts/123456",
        "http://0.0.0.0/posts/123456",
        "http://localhost/posts/123456",
        "http://[::1]/posts/123456",
        "http://[0:0:0:0:0:ffff:127.0.0.1]/posts/123456",
        "http://169.254.169.254/posts/123456",
        "http://metadata.google.internal/posts/123456",
        // Pathological hosts
        "https://evil.com/facebook.com/posts/123456",
        "https://attacker.com#facebook.com/posts/123456",
        "https://facebook.com.attacker.com:443/posts/123456",
    ];

    for hostile in hostile_urls {
        let res = parse_facebook_url(hostile);
        assert!(
            matches!(res, Err(FacebookExtractError::InvalidDomain(_))),
            "Hostile host in '{}' must be rejected with InvalidDomain, got: {:?}",
            hostile,
            res
        );
    }
}

#[test]
fn test_vector2_userinfo_spoofing() {
    // Userinfo spoofing: attacker puts facebook.com before @
    let spoofed = [
        "https://facebook.com@evil.com/posts/123456",
        "https://www.facebook.com:password@evil.com/posts/123456",
        "http://fb.com@attacker.org/watch/?v=123456",
    ];

    for s in spoofed {
        let res = parse_facebook_url(s);
        assert!(
            matches!(res, Err(FacebookExtractError::InvalidDomain(_))),
            "Userinfo spoofing URL '{}' must be rejected with InvalidDomain, got: {:?}",
            s,
            res
        );
    }
}

#[test]
fn test_vector2_non_http_schemes_rejected() {
    let forbidden_schemes = [
        "file:///etc/passwd",
        "file:///facebook.com/posts/123456",
        "gopher://facebook.com/posts/123456",
        "ftp://facebook.com/posts/123456",
        "javascript:alert(1)",
        "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
        "ws://facebook.com/posts/123456",
        "wss://facebook.com/posts/123456",
        "ldap://facebook.com/posts/123456",
        "ssh://git@facebook.com:user/repo.git",
        "mailto:post@facebook.com",
    ];

    for scheme_url in forbidden_schemes {
        let res = parse_facebook_url(scheme_url);
        assert!(
            matches!(res, Err(FacebookExtractError::InvalidInput(_))),
            "Forbidden scheme in '{}' must be rejected with InvalidInput, got: {:?}",
            scheme_url,
            res
        );
    }
}

// ============================================================================
// VECTOR 3: Null Bytes, Control Characters, & Malformed Payloads
// ============================================================================

#[test]
fn test_vector3_embedded_null_bytes_and_control_chars_in_ids_rejected() {
    // Any input with null bytes or control characters embedded inside the ID
    // must be strictly rejected by validate_post_id (returns Err(InvalidInput))
    let id_injections = [
        "https://www.facebook.com/posts/123\045",
        "https://www.facebook.com/posts/123\x0145",
        "https://www.facebook.com/posts/123\x1b45",
        "https://www.facebook.com/posts/123\x7f45",
        "\012345",
        "123\045",
        "123\x0145",
        "https://www.facebook.com/posts/123%0045",
        "https://www.facebook.com/watch/?v=123%0045",
        "https://www.facebook.com/reel/123\045",
    ];

    for input in id_injections {
        let res = parse_facebook_url(input);
        assert!(
            matches!(res, Err(FacebookExtractError::InvalidInput(_))),
            "Embedded control/null byte in ID '{:?}' must be rejected with InvalidInput, got: {:?}",
            input,
            res
        );
    }
}

#[test]
fn test_vector3_whatwg_sanitization_guarantees_no_null_leakage() {
    // In WHATWG URL parsing, trailing C0 controls (e.g. \0) or newlines are stripped.
    // If parse_facebook_url accepts such an input, it MUST guarantee that the extracted
    // ID and canonical_url contain ZERO null bytes, zero control characters, and validate cleanly.
    let sanitized_inputs = [
        "https://www.facebook.com/posts/12345\0",
        "https://www.facebook.com/posts/12345\n",
        "https://www.facebook.com/posts/12345\r\n",
    ];

    for input in sanitized_inputs {
        let res = parse_facebook_url(input);
        assert!(res.is_ok(), "Input should be parsed without panic: {:?}", input);
        let info = res.unwrap();
        assert!(!info.id.contains('\0'), "Extracted ID must never contain null bytes");
        assert!(!info.id.contains('\n'), "Extracted ID must never contain newlines");
        assert!(!info.canonical_url.contains('\0'), "Canonical URL must never contain null bytes");
        assert!(validate_post_id(&info.id).is_ok());
    }
}

#[test]
fn test_vector3_path_traversal_and_malformed_slashes() {
    let traversal_inputs = [
        "https://www.facebook.com/../../../etc/passwd",
        "https://www.facebook.com/posts/../../secret",
        "https://www.facebook.com/author/../posts/12345",
    ];

    for input in traversal_inputs {
        let res = parse_facebook_url(input);
        // Traversal path either resolves or gets rejected as invalid/unsupported, but MUST NOT PANIC
        match res {
            Ok(info) => {
                assert!(!info.id.is_empty());
                assert!(validate_post_id(&info.id).is_ok());
            }
            Err(e) => {
                assert!(
                    matches!(e, FacebookExtractError::UnsupportedUrl(_) | FacebookExtractError::InvalidInput(_))
                );
            }
        }
    }

    // Malformed protocol-relative and repeated slashes
    let malformed_slashes = ["//", "///", "////", "///posts/12345"];
    for slash in malformed_slashes {
        let res = parse_facebook_url(slash);
        assert!(res.is_err(), "Slash string '{}' must fail gracefully without panic", slash);
    }

    // Repeated slashes within valid path should parse cleanly
    let repeated_slashes = "https://www.facebook.com//////posts//////123456789//////";
    let res = parse_facebook_url(repeated_slashes);
    assert!(res.is_ok(), "Repeated slashes should be normalized without panic");
    assert_eq!(res.unwrap().id, "123456789");
}

// ============================================================================
// VECTOR 4: Tracking Parameters & Complex Query Strings & Multiple Fragments
// ============================================================================

#[test]
fn test_vector4_all_tracking_parameters_stripped() {
    let url = "https://www.facebook.com/zuck/posts/123456789?\
        fbclid=IwAR2x123abc&\
        mibextid=ZbWKwL&\
        rdid=x9y8z7&\
        utm_source=fb_feed&\
        utm_medium=cpc&\
        utm_campaign=summer_promo&\
        utm_term=sale&\
        utm_content=banner&\
        __cft__[0]=AZX_123456&\
        __tn__=%2CO%2CP-R&\
        paipv=0&\
        notif_t=feedback_reaction_generic&\
        notif_id=987654321&\
        ref=bookmarks&\
        refsrc=deprecated&\
        hc_ref=NEWSFEED&\
        _rdr";

    let res = parse_facebook_url(url).expect("URL with comprehensive tracking parameters should parse");
    assert_eq!(res.id, "123456789");
    assert_eq!(res.author_handle.as_deref(), Some("zuck"));
    assert_eq!(
        res.canonical_url,
        "https://www.facebook.com/zuck/posts/123456789/"
    );

    // Verify all tracking params are identified by is_tracking_param
    assert!(is_tracking_param("fbclid"));
    assert!(is_tracking_param("FBCLID"));
    assert!(is_tracking_param("mibextid"));
    assert!(is_tracking_param("MibExtId"));
    assert!(is_tracking_param("rdid"));
    assert!(is_tracking_param("utm_source"));
    assert!(is_tracking_param("utm_medium"));
    assert!(is_tracking_param("__cft__[0]"));
    assert!(is_tracking_param("__tn__"));
    assert!(is_tracking_param("paipv"));
    assert!(is_tracking_param("notif_t"));
    assert!(is_tracking_param("ref"));
    assert!(is_tracking_param("refsrc"));
    assert!(is_tracking_param("hc_ref"));
    assert!(is_tracking_param("_rdr"));

    // Verify non-tracking params are NOT stripped
    assert!(!is_tracking_param("v"));
    assert!(!is_tracking_param("story_fbid"));
    assert!(!is_tracking_param("id"));
    assert!(!is_tracking_param("fbid"));
}

#[test]
fn test_vector4_multiple_hash_fragments() {
    let urls_with_fragments = [
        "https://www.facebook.com/posts/123456789#frag1#frag2#frag3",
        "https://www.facebook.com/watch/?v=987654321#t=10#share",
        "https://www.facebook.com/reel/1122334455#comment_123#reply",
        "https://www.facebook.com/zuck/posts/123456789?fbclid=abc#header?query=param/subpath",
    ];

    for url in urls_with_fragments {
        let res = parse_facebook_url(url);
        assert!(res.is_ok(), "URL with multiple fragments '{}' should parse without panic", url);
        let info = res.unwrap();
        assert!(!info.id.contains('#'), "Fragment '#' must not leak into extracted ID: '{}'", info.id);
        assert!(!info.canonical_url.contains('#'), "Fragment '#' must not leak into canonical URL: '{}'", info.canonical_url);
    }
}

// ============================================================================
// VECTOR 5: Non-Post Facebook Paths Rejection
// ============================================================================

#[test]
fn test_vector5_non_post_paths_strictly_rejected() {
    let non_post_urls = [
        "https://www.facebook.com/marketplace",
        "https://www.facebook.com/marketplace/item/123456789/",
        "https://www.facebook.com/messages",
        "https://www.facebook.com/messages/t/123456789/",
        "https://www.facebook.com/friends",
        "https://www.facebook.com/friends/requests/",
        "https://www.facebook.com/notifications",
        "https://www.facebook.com/notifications/",
        "https://www.facebook.com/search",
        "https://www.facebook.com/search/top/?q=rustlang",
        "https://www.facebook.com/search/posts/?q=rustlang",
        "https://www.facebook.com/settings",
        "https://www.facebook.com/settings/account/",
        "https://www.facebook.com/events",
        "https://www.facebook.com/events/123456789/",
        "https://www.facebook.com/gaming",
        "https://www.facebook.com/ads",
        "https://www.facebook.com/saved",
        "https://www.facebook.com/bookmarks",
        "https://www.facebook.com/pages",
    ];

    for url in non_post_urls {
        let res = parse_facebook_url(url);
        assert!(
            matches!(res, Err(FacebookExtractError::UnsupportedUrl(_))),
            "Non-post path '{}' must be rejected with UnsupportedUrl, got: {:?}",
            url,
            res
        );
    }
}

#[test]
fn test_vector5_login_pages_rejected_as_login_wall() {
    let login_urls = [
        "https://www.facebook.com/login",
        "https://www.facebook.com/login/",
        "https://www.facebook.com/login.php",
        "https://www.facebook.com/login.php?next=https%3A%2F%2Fwww.facebook.com%2Fposts%2F123",
        "https://m.facebook.com/login.php",
    ];

    for url in login_urls {
        let res = parse_facebook_url(url);
        assert!(
            matches!(res, Err(FacebookExtractError::PrivateOrLoginWall(_))),
            "Login page URL '{}' must be rejected with PrivateOrLoginWall, got: {:?}",
            url,
            res
        );
    }
}

#[test]
fn test_vector5_profile_root_and_missing_ids_rejected() {
    // Single segment profile or page URLs
    let profile_urls = [
        "https://www.facebook.com/zuck",
        "https://www.facebook.com/zuck/",
        "https://www.facebook.com/BBCNews",
        "https://www.facebook.com/groups/rustaceans", // group root without post ID
    ];

    for url in profile_urls {
        let res = parse_facebook_url(url);
        assert!(
            matches!(res, Err(FacebookExtractError::UnsupportedUrl(_))),
            "Profile or group root URL '{}' must be rejected with UnsupportedUrl, got: {:?}",
            url,
            res
        );
    }

    // Missing IDs on valid post endpoints
    let missing_id_urls = [
        "https://www.facebook.com/posts/",
        "https://www.facebook.com/reel/",
        "https://www.facebook.com/reels/",
        "https://www.facebook.com/watch/",
        "https://www.facebook.com/video.php",
        "https://www.facebook.com/permalink.php",
        "https://www.facebook.com/story.php",
        "https://www.facebook.com/photo.php",
        "https://www.facebook.com/share/",
        "https://www.facebook.com/share/p/",
        "https://www.facebook.com/share/r/",
        "https://www.facebook.com/share/v/",
        "https://www.facebook.com/zuck/posts/",
        "https://www.facebook.com/zuck/videos/",
        "https://fb.watch/",
    ];

    for url in missing_id_urls {
        let res = parse_facebook_url(url);
        assert!(
            matches!(res, Err(FacebookExtractError::InvalidInput(_))),
            "Missing ID in endpoint URL '{}' must be rejected with InvalidInput, got: {:?}",
            url,
            res
        );
    }
}

// ============================================================================
// VECTOR 6: Accurate Extraction Invariants & Oracle Verification
// ============================================================================

#[test]
fn test_vector6_comprehensive_valid_extraction_matrix() {
    let test_cases = [
        // 1. Raw numeric post ID
        ("123456789012345", "123456789012345", "post", false, false),
        // 2. Raw pfbid token
        ("pfbid02AbCdEfGh123", "pfbid02AbCdEfGh123", "post", false, false),
        // 3. User post with numeric ID
        ("https://www.facebook.com/zuck/posts/123456789012345", "123456789012345", "post", false, false),
        // 4. User post with pfbid ID
        ("https://www.facebook.com/zuck/posts/pfbid02AbCdEfGh123", "pfbid02AbCdEfGh123", "post", false, false),
        // 5. Short post path /posts/{id}
        ("https://www.facebook.com/posts/123456789012345", "123456789012345", "post", false, false),
        // 6. Reel singular
        ("https://www.facebook.com/reel/987654321098765/", "987654321098765", "reel", false, true),
        // 7. Reels plural
        ("https://www.facebook.com/reels/987654321098765/", "987654321098765", "reel", false, true),
        // 8. Watch with ?v= query
        ("https://www.facebook.com/watch/?v=112233445566", "112233445566", "watch", false, true),
        // 9. Watch path /watch/{id}
        ("https://www.facebook.com/watch/112233445566/", "112233445566", "watch", false, true),
        // 10. Page videos /{author}/videos/{id}
        ("https://www.facebook.com/NASA/videos/5544332211/", "5544332211", "watch", false, true),
        // 11. video.php?v={id}
        ("https://www.facebook.com/video.php?v=9988776655", "9988776655", "watch", false, true),
        // 12. Shortened fb.watch link
        ("https://fb.watch/k7A8b9C_d1/", "k7A8b9C_d1", "watch", true, true),
        // 13. Mobile share post /share/p/{id}
        ("https://www.facebook.com/share/p/XyZ123abc/", "XyZ123abc", "post", true, false),
        // 14. Mobile share reel /share/r/{id}
        ("https://www.facebook.com/share/r/ReEl123abc/", "ReEl123abc", "reel", true, true),
        // 15. Mobile share video /share/v/{id}
        ("https://www.facebook.com/share/v/ViDeO123abc/", "ViDeO123abc", "watch", true, true),
        // 16. Generic share /share/{id}
        ("https://www.facebook.com/share/GenericSlug123/", "GenericSlug123", "share", true, false),
        // 17. Legacy permalink.php
        ("https://www.facebook.com/permalink.php?story_fbid=55667788&id=1000123", "55667788", "post", false, false),
        // 18. Legacy mobile story.php
        ("https://m.facebook.com/story.php?story_fbid=998877&id=1000456", "998877", "post", false, false),
        // 19. Photo.php with fbid
        ("https://www.facebook.com/photo.php?fbid=44332211", "44332211", "photo", false, false),
        // 20. Group posts path
        ("https://www.facebook.com/groups/12345678/posts/87654321/", "87654321", "post", false, false),
        // 21. Group permalink path
        ("https://www.facebook.com/groups/mygroup/permalink/99887766/", "99887766", "post", false, false),
        // 22. Mobile domain m.facebook.com
        ("https://m.facebook.com/zuck/posts/123456789", "123456789", "post", false, false),
        // 23. Web domain web.facebook.com
        ("https://web.facebook.com/zuck/posts/123456789", "123456789", "post", false, false),
        // 24. Short domain fb.com
        ("https://fb.com/posts/123456789", "123456789", "post", false, false),
        // 25. Protocol-relative scheme
        ("//www.facebook.com/posts/123456789", "123456789", "post", false, false),
        // 26. Schemeless URL auto-prefixing
        ("facebook.com/posts/123456789", "123456789", "post", false, false),
    ];

    for (raw, expected_id, expected_kind, expected_redirect, expected_video) in test_cases {
        let res = parse_facebook_url(raw);
        assert!(
            res.is_ok(),
            "Expected valid parse for '{}', got error: {:?}",
            raw,
            res.err()
        );
        let info = res.unwrap();
        assert_eq!(info.id, expected_id, "ID mismatch for '{}'", raw);
        assert_eq!(info.kind, expected_kind, "Kind mismatch for '{}'", raw);
        assert_eq!(info.is_redirect_needed, expected_redirect, "Redirect flag mismatch for '{}'", raw);
        assert_eq!(info.is_video, expected_video, "is_video flag mismatch for '{}'", raw);
        assert!(info.canonical_url.starts_with("https://"), "Canonical URL must start with https:// for '{}'", raw);
        assert!(!info.id.is_empty(), "ID must not be empty for '{}'", raw);
        assert!(validate_post_id(&info.id).is_ok(), "ID must conform to validate_post_id for '{}'", raw);
    }
}

// ============================================================================
// VECTOR 7: High-Iteration Stress Fuzzer & Invariant Property Verifier
// ============================================================================

#[test]
fn test_vector7_fuzz_random_byte_sequences_zero_panics() {
    // 2,000 randomized hostile string sequences
    let mut state: u64 = 0x853c49e6748fea9b;
    let mut rng = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    let prefixes = [
        "https://www.facebook.com/",
        "https://fb.watch/",
        "https://m.facebook.com/",
        "http://facebook.com/",
        "//facebook.com/",
        "",
    ];

    let charset = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789/-_?=&%.:#\0\t\r\n\x01\x1b";

    for _i in 0..2_000 {
        let prefix = prefixes[(rng() as usize) % prefixes.len()];
        let len = (rng() % 120) as usize;
        let mut random_body = String::with_capacity(len);
        for _ in 0..len {
            let byte = charset[(rng() as usize) % charset.len()];
            random_body.push(byte as char);
        }

        let full_candidate = format!("{}{}", prefix, random_body);

        // Invariant: parse_facebook_url MUST NEVER PANIC
        let res = parse_facebook_url(&full_candidate);
        if let Ok(info) = res {
            // Invariant: any Ok result must have valid canonical URL and non-empty valid ID
            assert!(info.canonical_url.starts_with("https://"), "Canonical URL must start with https://, got: {}", info.canonical_url);
            assert!(!info.id.is_empty(), "Extracted ID must never be empty");
            assert!(validate_post_id(&info.id).is_ok(), "Extracted ID '{}' must be valid alphanumeric/pfbid", info.id);
            assert!(["post", "reel", "watch", "photo", "share"].contains(&info.kind.as_str()), "Extracted kind must be one of known kinds, got: {}", info.kind);
        }
    }
}
