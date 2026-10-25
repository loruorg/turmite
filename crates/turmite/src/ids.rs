use rand::RngExt;

const ALPHANUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

fn random_string(alphabet: &[u8], len: usize) -> String {
    let mut rng = rand::rng();
    (0..len).map(|_| alphabet[rng.random_range(0..alphabet.len())] as char).collect()
}

/// Random public token for URLs (about 143 bits).
pub fn token() -> String {
    random_string(ALPHANUM, 24)
}

/// Random numeric id, 10 digits, no leading zero.
pub fn numeric_id() -> String {
    let mut rng = rand::rng();
    rng.random_range(1_000_000_000u64..10_000_000_000).to_string()
}

/// Numeric one-time code with uniform digits.
pub fn new_numeric_code(digits: u32) -> String {
    random_string(b"0123456789", digits as usize)
}

/// API token `{prefix}{40 chars}`. Returns (token, last 4 characters).
pub fn new_api_token(prefix: &str) -> (String, String) {
    let token = format!("{prefix}{}", random_string(ALPHANUM, 40));
    let last4 = token[token.len() - 4..].to_string();
    (token, last4)
}
