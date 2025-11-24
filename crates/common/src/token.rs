use rand::distr::{Alphanumeric, SampleString};
use secrecy::{ExposeSecret, SecretSlice, SecretString};
use sha2::{Digest, Sha256};

const TOKEN_LENGTH: usize = 64;

pub struct HashedToken(SecretSlice<u8>);

impl HashedToken {
    pub fn parse(plaintext: &str) -> Self {
        let sha256 = Self::hash(plaintext).into();
        Self(sha256)
    }

    pub fn hash(plaintext: &str) -> Vec<u8> {
        Sha256::digest(plaintext.as_bytes()).as_slice().to_vec()
    }
}

impl ExposeSecret<[u8]> for HashedToken {
    fn expose_secret(&self) -> &[u8] {
        self.0.expose_secret()
    }
}

pub struct PlainToken(SecretString);

impl PlainToken {
    pub fn generate() -> Self {
        let plaintext = generate_secure_alphanumeric_string(TOKEN_LENGTH).into();

        Self(plaintext)
    }

    pub fn hashed(&self) -> HashedToken {
        let sha256 = HashedToken::hash(self.expose_secret()).into();
        HashedToken(sha256)
    }
}

impl ExposeSecret<str> for PlainToken {
    fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

fn generate_secure_alphanumeric_string(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), len)
}
