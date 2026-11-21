//! Tracking-parameter stripping for whole-payload URL clips.
//!
//! A copied link is usually decorated with parameters that identify whoever
//! shared it — YouTube's `si`, Amazon's `ref_`, a newsletter's `mc_eid` — and the
//! page rarely needs any of them. Kept cleans them before the clip is stored and
//! keeps the address exactly as it was copied beside the clean one, so "Copy
//! uncleaned URL" can still hand the original back.
//!
//! Three rules keep this from breaking links:
//!
//! * Only whole-payload URLs are touched. A URL inside a longer text payload has
//!   no clear boundary, and a false positive there is invisible to the user.
//! * Names are matched exactly, plus a few distinctive prefixes. A broad regex
//!   such as ClearURLs' `li[a-z]{2}` would eat real parameters on the long tail of
//!   sites that no test covers.
//! * [`GLOBAL_KEEP`] wins over every rule, and sign-in, checkout, reset, verify
//!   and unsubscribe addresses are left completely alone. Those are the links
//!   where dropping a parameter means a dead link instead of a lost click id.
//!
//! Rule sources: AdGuard's URL Tracking Protection filter, ClearURLs' provider
//! list and Firefox's query-stripping remote settings. Affiliate and personal-data
//! parameters (Amazon `tag`, Etsy `click_key`, `email=`) are deliberately absent:
//! stripping those changes what the link means for someone else, so they belong
//! behind their own opt-in.

use crate::models::{ClipContent, NewClip};

/// One link with its tracking parameters removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
    pub url: String,
    /// The parameter names that were dropped, in the order they appeared.
    pub removed: Vec<String>,
}

/// Drops any rule that matches, on any host.
///
/// These never carry useful state: they identify a click, a campaign click, an
/// email recipient or an analytics session, and a page that needs one of them to
/// render is broken for everyone who types the address by hand too.
const GENERIC_DROP: &[&str] = &[
    // Ad click ids.
    "fbclid",
    "gclid",
    "gclsrc",
    "dclid",
    "gbraid",
    "wbraid",
    "gad_source",
    "msclkid",
    "yclid",
    "ysclid",
    "twclid",
    "ttclid",
    "tgclid",
    "srsltid",
    "epik",
    "irclickid",
    "irgwc",
    "rb_clickid",
    "s_cid",
    "wickedid",
    "_openstat",
    // Email and CRM tracking.
    "mkt_tok",
    "_hsenc",
    "_hsmi",
    "__hstc",
    "__hssc",
    "__hsfp",
    "hsctatracking",
    "mc_cid",
    "mc_eid",
    "ml_subscriber",
    "ml_subscriber_hash",
    "vero_id",
    "vero_conv",
    "oly_anon_id",
    "oly_enc_id",
    "__s",
    "eml-name",
    "eml-mediaplan",
    "eml-publisher",
    // Analytics and cross-domain linkers.
    "_ga",
    "_gl",
    "_bdadid",
    "_branch_match_id",
    "_branch_referrer",
    "gs_l",
    "__twitter_impression",
];

/// Prefix rules, kept to the three families that are genuinely distinctive.
///
/// `itm_` and `pk_` are absent on purpose: both collide with ordinary shopping and
/// payment parameter names, and the whole point of an exact-name table is that a
/// site nobody has looked at cannot be caught by accident.
const GENERIC_PREFIXES: &[&str] = &["utm_", "mtm_", "hsa_"];

/// Parameters no rule may ever drop.
///
/// These name the thing the link points at, carry a signature that authorises the
/// request, or tell the page where to go next. `t` and `s` are absent because the
/// only rules that drop them are scoped to X, where they are share timestamps.
const GLOBAL_KEEP: &[&str] = &[
    "v",
    "id",
    "p",
    "q",
    "u",
    "url",
    "target",
    "next",
    "return",
    "return_to",
    "returnto",
    "continue",
    "redirect",
    "redirect_uri",
    "redirect_url",
    "token",
    "key",
    "auth",
    "sig",
    "signature",
    "expires",
    "policy",
    "code",
    "state",
    "hash",
    "page",
    "sort",
    "filter",
    "hl",
    "lang",
    "list",
    "index",
];

/// Hosts whose whole payload is a credential or a transaction.
///
/// Cleaning any of these is a Level 1 mistake: the page 404s or the link stops
/// authorising instead of merely losing an attribution parameter.
const SENSITIVE_HOST_PREFIXES: &[&str] = &[
    "accounts.",
    "login.",
    "signin.",
    "auth.",
    "checkout.",
    "sso.",
    "secure.",
];

const SENSITIVE_PATH_PARTS: &[&str] = &[
    "/login",
    "/signin",
    "/oauth",
    "/checkout",
    "/password",
    "/reset",
    "/verify",
    "/activate",
    "/unsubscribe",
    "/cart",
];

/// Amazon's parameters, shared by every regional storefront and the shorteners.
const AMAZON: &[&str] = &[
    "pd_rd_r",
    "pd_rd_w",
    "pd_rd_wg",
    "pf_rd_p",
    "pf_rd_r",
    "pf_rd_s",
    "pf_rd_t",
    "pf_rd_i",
    "pf_rd_m",
    "qid",
    "sr",
    "crid",
    "dchild",
    "th",
    "ref_",
    "refrid",
    "linkcode",
    "creative",
    "creativeasin",
    "ascsubtag",
    "aaxitk",
    "content-id",
    "smid",
    "spia",
    "_encoding",
    "rnid",
    "dib",
    "dib_tag",
];

const EBAY: &[&str] = &["_trkparms", "_trksid", "_from"];

const ALIEXPRESS: &[&str] = &[
    "spm",
    "scm",
    "gps-id",
    "algo_pvid",
    "algo_expid",
    "btsid",
    "ws_ab_test",
    "pdp_npi",
    "terminal_id",
    "cv",
    "af",
    "sk",
    "dp",
];

/// Every regional storefront, written out rather than wildcarded.
///
/// A rule keyed on "the label before the TLD is `amazon`" would also match
/// `amazon.evil.example`. Listing the tails costs one line per country and fails
/// safe: an unknown storefront simply keeps its parameters.
const AMAZON_TAILS: &[&str] = &[
    "com", "co.uk", "de", "fr", "it", "es", "ca", "com.au", "co.jp", "in", "cn", "com.br",
    "com.mx", "nl", "se", "pl", "com.tr", "ae", "sa", "eg", "sg", "ie", "be",
];

const EBAY_TAILS: &[&str] = &[
    "com", "co.uk", "de", "fr", "it", "es", "ca", "com.au", "at", "be", "ch", "ie", "nl", "pl",
    "com.hk", "com.sg", "ph", "com.my", "com.mx", "com.br",
];

const ALIEXPRESS_TAILS: &[&str] = &[
    "com", "us", "ru", "es", "fr", "pt", "it", "de", "nl", "pl", "tr", "ar", "th", "vi", "id",
    "com.br", "jp", "cl", "co",
];

/// Parameters dropped on a host or any subdomain of it.
const HOST_RULES: &[(&str, &[&str])] = &[
    (
        "youtube.com",
        &[
            "si",
            "pp",
            "feature",
            "kw",
            "embeds_referring_euri",
            "embeds_referring_origin",
            "source_ve_path",
            "ab_channel",
        ],
    ),
    (
        "youtube-nocookie.com",
        &[
            "si",
            "pp",
            "feature",
            "kw",
            "embeds_referring_euri",
            "embeds_referring_origin",
            "source_ve_path",
        ],
    ),
    ("youtu.be", &["si", "pp", "feature", "kw"]),
    (
        "x.com",
        &["s", "t", "ref_src", "ref_url", "src", "cn", "twclid"],
    ),
    (
        "twitter.com",
        &["s", "t", "ref_src", "ref_url", "src", "cn", "twclid"],
    ),
    (
        "facebook.com",
        &[
            "fbclid",
            "mibextid",
            "rdid",
            "sfnsn",
            "eav",
            "paipv",
            "__tn__",
            "__cft__",
            "hc_ref",
            "hc_location",
            "fref",
            "comment_tracking",
            "dti",
            "app",
            "wtsid",
            "rdc",
            "_rdr",
            "_nc_x",
            "tracking",
            "referral_code",
            "referral_story_type",
        ],
    ),
    (
        "fb.com",
        &["fbclid", "mibextid", "rdid", "sfnsn", "eav", "paipv"],
    ),
    ("instagram.com", &["igshid", "igsh"]),
    ("threads.net", &["xmt", "igshid", "share_id"]),
    ("threads.com", &["xmt", "igshid", "share_id"]),
    (
        "reddit.com",
        &[
            "share_id",
            "rdt",
            "correlation_id",
            "ref_campaign",
            "ref_source",
            "_branch_match_id",
            "$deep_link",
            "$3p",
            "$original_url",
        ],
    ),
    ("redd.it", &["share_id", "rdt", "correlation_id"]),
    (
        "linkedin.com",
        &[
            "trk",
            "trackingid",
            "refid",
            "lipi",
            "midtoken",
            "midsig",
            "trkinfo",
            "original_referer",
            "originalsubdomain",
        ],
    ),
    ("lnkd.in", &["trk", "trackingid"]),
    (
        "tiktok.com",
        &[
            "u_code",
            "preview_pb",
            "_d",
            "_t",
            "_r",
            "timestamp",
            "user_id",
            "share_app_name",
            "share_iid",
            "share_link_id",
            "share_token",
            "source",
            "is_from_webapp",
            "sender_device",
            "refer",
            "enter_from",
        ],
    ),
    ("spotify.com", &["si"]),
    ("amzn.to", AMAZON),
    ("amzn.eu", AMAZON),
    ("amzn.asia", AMAZON),
    (
        "google.com",
        &[
            "ved", "ei", "sa", "source", "sxsrf", "rlz", "usg", "uact", "cd", "cad", "aqs", "bih",
            "biw", "dpr", "client", "sourceid", "sca_esv", "iflsig", "esrc", "vet", "ie", "je",
            "dcr", "sei", "gws_rd", "usqp",
        ],
    ),
    ("bing.com", &["cvid", "sk", "sp", "sc", "qs", "qp"]),
    ("msn.com", &["cvid", "ocid"]),
    ("microsoft.com", &["cvid", "epi"]),
    ("substack.com", &["r", "triedredirect"]),
    ("medium.com", &["source"]),
    ("twitch.tv", &["tt_medium", "tt_content", "sr"]),
    ("steampowered.com", &["snr"]),
    ("steamcommunity.com", &["snr"]),
    ("etsy.com", &["ref", "organic_search_click"]),
    (
        "walmart.com",
        &["u1", "athbdg", "athcpid", "athpgid", "athznid"],
    ),
    ("indeed.com", &["from", "alid", "tk", "vjs"]),
];

/// Parameters dropped on a brand's regional domains, keyed by the label before
/// the country tail (`amazon` + `co.uk`).
const BRAND_RULES: &[(&str, &[&str], &[&str])] = &[
    ("amazon", AMAZON_TAILS, AMAZON),
    ("ebay", EBAY_TAILS, EBAY),
    ("aliexpress", ALIEXPRESS_TAILS, ALIEXPRESS),
];

/// Removes tracking parameters from a whole-payload URL.
///
/// Answers `None` when the payload is not a single http(s) address, when it is a
/// sensitive address, or when no rule matched — the last one is what keeps an
/// already-clean link byte-for-byte identical.
pub fn clean(text: &str) -> Option<Cleaned> {
    let trimmed = text.trim();
    if !crate::color::is_url(trimmed) {
        return None;
    }
    let (head, query, fragment) = split_query(trimmed)?;
    let host = host_of(head);
    if is_sensitive(head, host) {
        return None;
    }
    let scoped = host.and_then(scoped_drop_list);

    let mut removed = Vec::new();
    let mut kept: Vec<&str> = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let name = pair.split_once('=').map_or(pair, |(name, _)| name);
        let lower = name.to_ascii_lowercase();
        if should_drop(&lower, scoped) {
            removed.push(name.to_string());
        } else {
            kept.push(pair);
        }
    }
    if removed.is_empty() {
        return None;
    }

    let mut url = String::with_capacity(trimmed.len());
    url.push_str(head);
    if !kept.is_empty() {
        url.push('?');
        url.push_str(&kept.join("&"));
    }
    url.push_str(fragment);
    Some(Cleaned { url, removed })
}

/// Cleans a clip's payload in place, returning the address it replaced.
///
/// Every derived field is rebuilt, because the stored row is what the list, the
/// search index and the de-duplication key all agree on. The rich flavours are
/// dropped: a browser's `CF_HTML` still holds the decorated address in its `href`,
/// and pasting that over a clean plain text would hand the trackers straight back.
pub fn clean_clip(new: &mut NewClip) -> Option<String> {
    let ClipContent::Text(text) = &new.content else {
        return None;
    };
    let cleaned = clean(text)?;
    let original = text.trim().to_string();
    if cleaned.url == original {
        return None;
    }

    new.content = ClipContent::Text(cleaned.url);
    let ClipContent::Text(body) = &new.content else {
        return None;
    };
    new.content_hash = crate::hashing::of_content(&new.content);
    let collapsed: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    new.preview = collapsed.chars().take(320).collect();
    new.search_text = crate::search::normalize(body);
    new.html = None;
    new.rtf = None;
    Some(original)
}

fn should_drop(name: &str, scoped: Option<&'static [&'static str]>) -> bool {
    if GLOBAL_KEEP.contains(&name) {
        return false;
    }
    if let Some(list) = scoped
        && list
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    {
        return true;
    }
    if GENERIC_DROP
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
    {
        return true;
    }
    GENERIC_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Splits `url` into the part before `?`, the query, and the `#fragment`.
///
/// A `?` after the `#` belongs to the fragment, which this module never rewrites:
/// hash routes are usually the application's own state, not the site's analytics.
fn split_query(url: &str) -> Option<(&str, &str, &str)> {
    let question = url.find('?')?;
    let after = &url[question + 1..];
    Some(match after.find('#') {
        Some(hash) => (&url[..question], &after[..hash], &after[hash..]),
        None => (&url[..question], after, ""),
    })
}

fn host_of(head: &str) -> Option<&str> {
    let rest = head.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    // Credentials are rare in a copied link, but a host rule must not read them.
    let host_port = authority.rsplit('@').next()?;
    if host_port.starts_with('[') {
        // An IPv6 literal has no registrable name to match against.
        return None;
    }
    Some(host_port.split(':').next().unwrap_or(host_port))
}

fn is_sensitive(head: &str, host: Option<&str>) -> bool {
    if let Some(host) = host {
        let host = host.to_ascii_lowercase();
        if SENSITIVE_HOST_PREFIXES
            .iter()
            .any(|prefix| host.starts_with(prefix))
        {
            return true;
        }
    }
    let path = head.split_once("://").map(|(_, rest)| rest).unwrap_or(head);
    let path = path.find('/').map_or("", |slash| &path[slash..]);
    let path = path.to_ascii_lowercase();
    SENSITIVE_PATH_PARTS.iter().any(|part| path.contains(part))
}

fn scoped_drop_list(host: &str) -> Option<&'static [&'static str]> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    for (pattern, drops) in HOST_RULES {
        if host == *pattern {
            return Some(drops);
        }
        // Compare on a label boundary: `youtube.com.evil.example` ends with the
        // pattern's characters but is a different site.
        if host.len() > pattern.len()
            && host.ends_with(pattern)
            && host.as_bytes()[host.len() - pattern.len() - 1] == b'.'
        {
            return Some(drops);
        }
    }

    let rest = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("m."))
        .or_else(|| host.strip_prefix("smile."))
        .unwrap_or(&host);
    for (brand, tails, drops) in BRAND_RULES {
        if let Some(tail) = rest
            .strip_prefix(brand)
            .and_then(|next| next.strip_prefix('.'))
            && tails.contains(&tail)
        {
            return Some(drops);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A false positive here is a broken link, so the negative cases matter as
    /// much as the positive ones.
    fn cleaned(input: &str) -> Option<String> {
        clean(input).map(|result| result.url)
    }

    #[test]
    fn youtube_share_id_goes_and_the_timestamp_stays() {
        assert_eq!(
            cleaned("https://youtu.be/dQw4w9WgXcQ?si=Q3pT1xYc8Zk").as_deref(),
            Some("https://youtu.be/dQw4w9WgXcQ")
        );
        assert_eq!(
            cleaned("https://youtu.be/dQw4w9WgXcQ?t=42&si=Q3pT1xYc8Zk").as_deref(),
            Some("https://youtu.be/dQw4w9WgXcQ?t=42")
        );
        assert_eq!(
            cleaned(
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL123&index=4&t=30s&feature=share"
            )
            .as_deref(),
            Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL123&index=4&t=30s")
        );
    }

    #[test]
    fn service_rules_drop_only_their_own_parameters() {
        assert_eq!(
            cleaned("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=a1b2c3").as_deref(),
            Some("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC")
        );
        assert_eq!(
            cleaned("https://x.com/user/status/1?s=20&t=abc").as_deref(),
            Some("https://x.com/user/status/1")
        );
        assert_eq!(
            cleaned("https://www.reddit.com/r/rust/comments/abc/x/?share_id=1&sort=top").as_deref(),
            Some("https://www.reddit.com/r/rust/comments/abc/x/?sort=top")
        );
        assert_eq!(
            cleaned("https://www.linkedin.com/posts/x?trk=public_post&trackingId=abc").as_deref(),
            Some("https://www.linkedin.com/posts/x")
        );
        assert_eq!(
            cleaned("https://www.instagram.com/p/abc/?igsh=MXk3&img_index=2").as_deref(),
            Some("https://www.instagram.com/p/abc/?img_index=2")
        );
        assert_eq!(
            cleaned("https://www.tiktok.com/@user/video/123?_r=1&_t=abc&is_from_webapp=1")
                .as_deref(),
            Some("https://www.tiktok.com/@user/video/123")
        );
    }

    #[test]
    fn amazon_covers_regional_stores_but_not_lookalike_hosts() {
        assert_eq!(
            cleaned("https://www.amazon.co.uk/dp/B08N5WRWNW?th=1&psc=1&ref_=abc").as_deref(),
            Some("https://www.amazon.co.uk/dp/B08N5WRWNW?psc=1")
        );
        assert_eq!(
            cleaned("https://smile.amazon.com/dp/B08N5WRWNW?qid=1&k=usb").as_deref(),
            Some("https://smile.amazon.com/dp/B08N5WRWNW?k=usb")
        );
        assert_eq!(
            cleaned("https://www.amazon.com/dp/B08N5WRWNW?sr=8-1").as_deref(),
            Some("https://www.amazon.com/dp/B08N5WRWNW")
        );
        assert_eq!(cleaned("https://notamazon.co.uk/dp/B1?th=1"), None);
        assert_eq!(cleaned("https://amazon.evil.example/dp/B1?th=1"), None);
    }

    #[test]
    fn other_marketplaces_keep_their_affiliate_parameters() {
        // `mkcid`/`campid` are eBay Partner Network tags and `aff_*` are
        // AliExpress's: dropping them takes money from someone, so they wait for
        // the opt-in tier.
        assert_eq!(
            cleaned("https://www.ebay.co.uk/itm/123?_trkparms=abc&mkcid=1").as_deref(),
            Some("https://www.ebay.co.uk/itm/123?mkcid=1")
        );
        assert_eq!(
            cleaned("https://www.aliexpress.com/item/1.html?spm=a2g0o&aff_fcid=abc").as_deref(),
            Some("https://www.aliexpress.com/item/1.html?aff_fcid=abc")
        );
        assert_eq!(
            cleaned("https://www.etsy.com/listing/1?ref=shop_home&click_key=abc").as_deref(),
            Some("https://www.etsy.com/listing/1?click_key=abc")
        );
    }

    #[test]
    fn search_engines_keep_the_query_and_drop_the_rest() {
        assert_eq!(
            cleaned("https://www.google.com/search?q=rust&oq=rust&sourceid=chrome&ie=UTF-8")
                .as_deref(),
            Some("https://www.google.com/search?q=rust&oq=rust")
        );
        assert_eq!(
            cleaned("https://www.google.com/search?q=rust&tbm=isch&hl=en&ved=0ah").as_deref(),
            Some("https://www.google.com/search?q=rust&tbm=isch&hl=en")
        );
        assert_eq!(
            cleaned("https://www.bing.com/search?q=rust&cvid=abc&qs=n&form=QBLH").as_deref(),
            Some("https://www.bing.com/search?q=rust&form=QBLH")
        );
    }

    #[test]
    fn generic_prefixes_and_click_ids_go_everywhere() {
        assert_eq!(
            cleaned("https://example.com/page?utm_source=news&utm_medium=email&keep=1").as_deref(),
            Some("https://example.com/page?keep=1")
        );
        assert_eq!(
            cleaned("https://example.com/page?fbclid=abc&mtm_campaign=x").as_deref(),
            Some("https://example.com/page")
        );
    }

    #[test]
    fn keep_list_wins_over_every_rule() {
        // `token` and `state` are what an OAuth or reset link authorises with, and
        // `code`/`sig` are signatures. None of them may ever be treated as tracking.
        assert_eq!(
            cleaned("https://example.com/cb?token=abc&utm_source=x").as_deref(),
            Some("https://example.com/cb?token=abc")
        );
        assert_eq!(clean("https://example.com/cb?state=1&sig=2"), None);
    }

    #[test]
    fn sensitive_hosts_and_paths_are_never_touched() {
        for url in [
            "https://accounts.google.com/o/oauth2/auth?utm_source=x",
            "https://login.example.com/?utm_source=x",
            "https://example.com/login?utm_source=x",
            "https://example.com/account/reset?email=a@b.c&utm_source=x",
            "https://example.com/checkout?utm_source=x",
            "https://example.com/list/unsubscribe?utm_source=x",
            "https://shop.example/cart?utm_campaign=x",
        ] {
            assert_eq!(cleaned(url), None, "{url} must be left alone");
        }
    }

    #[test]
    fn hosts_are_matched_on_label_boundaries() {
        // A host that merely contains a brand name is a different site.
        assert_eq!(
            cleaned("https://youtube.com.evil.example/watch?v=1&si=2"),
            None
        );
        assert_eq!(
            cleaned("https://youtube.com/watch?v=1&si=2").as_deref(),
            Some("https://youtube.com/watch?v=1")
        );
    }

    #[test]
    fn query_rewriting_preserves_bytes_and_order() {
        assert_eq!(
            cleaned("https://example.com/a?b=%2F%26&utm_source=x&a=1").as_deref(),
            Some("https://example.com/a?b=%2F%26&a=1")
        );
        // `&` inside a value is percent-encoded, so it is not a separator, and a
        // redirect target is never rewritten.
        assert_eq!(
            cleaned("https://example.com/r?next=https://x.test/?utm_source=y"),
            None
        );
        // Duplicate names are all dropped, and the fragment survives untouched.
        assert_eq!(
            cleaned("https://example.com/a?utm_source=a&utm_source=b#frag?utm_source=c").as_deref(),
            Some("https://example.com/a#frag?utm_source=c")
        );
        // A query that was entirely tracking leaves no bare `?`.
        assert_eq!(
            cleaned("https://example.com/a?utm_source=a#top").as_deref(),
            Some("https://example.com/a#top")
        );
    }

    #[test]
    fn only_single_line_http_urls_are_considered() {
        assert_eq!(cleaned("hello"), None);
        assert_eq!(cleaned("ftp://example.com/a?utm_source=x"), None);
        assert_eq!(cleaned("see https://example.com/a?utm_source=x"), None);
        assert_eq!(
            cleaned("https://example.com/a?utm_source=x\nsecond line"),
            None
        );
        assert_eq!(cleaned("https://example.com/a"), None);
    }

    #[test]
    fn cleaning_is_idempotent() {
        let url = "https://youtu.be/dQw4w9WgXcQ?si=abc&t=5";
        let once = clean(url).expect("first pass cleans");
        assert_eq!(
            clean(&once.url),
            None,
            "a cleaned link has nothing left to do"
        );
    }

    #[test]
    fn clean_clip_reports_the_address_it_replaced() {
        let mut new = NewClip {
            content: ClipContent::Text("https://youtu.be/dQw4w9WgXcQ?si=abc".into()),
            content_hash: crate::hashing::of_str("https://youtu.be/dQw4w9WgXcQ?si=abc"),
            source_app: None,
            search_text: String::new(),
            preview: String::new(),
            html: Some("<a href=\"https://youtu.be/dQw4w9WgXcQ?si=abc\">x</a>".into()),
            rtf: None,
        };
        let original = clean_clip(&mut new).expect("trackers were present");
        assert_eq!(original, "https://youtu.be/dQw4w9WgXcQ?si=abc");
        let ClipContent::Text(body) = &new.content else {
            panic!("still text");
        };
        assert_eq!(body, "https://youtu.be/dQw4w9WgXcQ");
        assert_eq!(new.content_hash, crate::hashing::of_str(body));
        assert_eq!(new.preview, *body);
        assert!(new.html.is_none(), "the dirty href must not survive");

        let mut already_clean = NewClip {
            content: ClipContent::Text("https://youtu.be/dQw4w9WgXcQ".into()),
            content_hash: String::new(),
            source_app: None,
            search_text: String::new(),
            preview: String::new(),
            html: None,
            rtf: None,
        };
        assert_eq!(clean_clip(&mut already_clean), None);
    }
}
