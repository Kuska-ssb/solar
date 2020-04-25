use super::error::SolarResult;
use kuska_ssb::crypto::ToSodiumObject;
use kuska_ssb::crypto::ToSsbId;
use kuska_ssb::keystore::OwnedIdentity;

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub id: String,
    pub secret: String,
    pub friends: Vec<String>,
}

impl Config {
    pub fn create() -> Self {
        let OwnedIdentity { id, sk, .. } = OwnedIdentity::create();
        Config {
            id: id.clone(),
            secret: sk.to_ssb_id(),
            friends: Vec::new(),
        }
    }
    pub fn to_toml(&self) -> SolarResult<Vec<u8>> {
        Ok(toml::to_vec(&self)?)
    }
    pub fn from_toml(s: &[u8]) -> SolarResult<Self> {
        Ok(toml::from_slice::<Config>(s)?)
    }
    pub fn owned_identity(&self) -> SolarResult<OwnedIdentity> {
        Ok(OwnedIdentity {
            id: self.id.clone(),
            pk: self.id[1..].to_ed25519_pk()?,
            sk: self.secret.to_ed25519_sk()?,
        })
    }
}
