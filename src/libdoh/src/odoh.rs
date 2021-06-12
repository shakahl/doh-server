use crate::constants::ODOH_KEY_ROTATION_SECS;
use crate::errors::DoHError;
use arc_swap::ArcSwap;

use odoh_rs::{
    Deserialize, ObliviousDoHConfig, ObliviousDoHConfigs, ObliviousDoHKeyPair, ObliviousDoHMessage,
    ObliviousDoHMessagePlaintext, OdohSecret, ResponseNonce, Serialize,
};
use rand::Rng;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime;

#[derive(Clone)]
pub struct ODoHPublicKey {
    key: ObliviousDoHKeyPair,
    serialized_configs: Vec<u8>,
}

impl fmt::Debug for ODoHPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ODoHPublicKey").finish()
    }
}

#[derive(Clone, Debug)]
pub struct ODoHQueryContext {
    query: ObliviousDoHMessagePlaintext,
    secret: OdohSecret,
}

impl ODoHPublicKey {
    pub fn new() -> Result<ODoHPublicKey, DoHError> {
        let key_pair = ObliviousDoHKeyPair::new(&mut rand::thread_rng());
        let config = ObliviousDoHConfig::from(key_pair.public().clone());
        let mut serialized_configs = Vec::new();
        ObliviousDoHConfigs::from(vec![config.clone()])
            .serialize(&mut serialized_configs)
            .map_err(|e| DoHError::ODoHConfigError(e.into()))?;
        Ok(ODoHPublicKey {
            key: key_pair,
            serialized_configs: serialized_configs,
        })
    }

    pub fn config(self) -> Vec<u8> {
        self.serialized_configs
    }

    pub async fn decrypt_query(
        self,
        encrypted_query: Vec<u8>,
    ) -> Result<(Vec<u8>, ODoHQueryContext), DoHError> {
        let odoh_query = ObliviousDoHMessage::deserialize(&mut bytes::Bytes::from(encrypted_query))
            .map_err(|_| DoHError::InvalidData)?;
        match self.key.public().identifier() {
            Ok(key_id) => {
                if !key_id.eq(&odoh_query.key_id()) {
                    return Err(DoHError::StaleKey);
                }
            }
            Err(_) => return Err(DoHError::InvalidData),
        };
        let (query, server_secret) = match odoh_rs::decrypt_query(&odoh_query, &self.key) {
            Ok((pq, ss)) => (pq, ss),
            Err(_) => return Err(DoHError::InvalidData),
        };
        let context = ODoHQueryContext {
            query: query.clone(),
            secret: server_secret,
        };
        let mut query_bytes = Vec::new();
        query
            .serialize(&mut query_bytes)
            .map_err(|_| DoHError::InvalidData)?;
        Ok((query_bytes, context))
    }
}

impl ODoHQueryContext {
    pub async fn encrypt_response(self, response_body: Vec<u8>) -> Result<Vec<u8>, DoHError> {
        let response_nonce = rand::thread_rng().gen::<ResponseNonce>();
        let response_body_ =
            ObliviousDoHMessagePlaintext::deserialize(&mut bytes::Bytes::from(response_body))
                .map_err(|_| DoHError::InvalidData)?;
        let encrypted_response =
            odoh_rs::encrypt_response(&self.query, &response_body_, self.secret, response_nonce)
                .map_err(|_| DoHError::InvalidData)?;
        let mut encrypted_response_bytes = Vec::new();
        encrypted_response
            .serialize(&mut encrypted_response_bytes)
            .map_err(|_| DoHError::InvalidData)?;
        Ok(encrypted_response_bytes)
    }
}

#[derive(Clone, Debug)]
pub struct ODoHRotator {
    key: Arc<ArcSwap<ODoHPublicKey>>,
}

impl ODoHRotator {
    pub fn new(runtime_handle: runtime::Handle) -> Result<ODoHRotator, DoHError> {
        let odoh_key = match ODoHPublicKey::new() {
            Ok(key) => Arc::new(ArcSwap::from_pointee(key)),
            Err(e) => panic!("ODoH key rotation error: {}", e),
        };

        let current_key = Arc::clone(&odoh_key);

        runtime_handle.clone().spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(ODOH_KEY_ROTATION_SECS.into())).await;
                match ODoHPublicKey::new() {
                    Ok(key) => {
                        current_key.store(Arc::new(key));
                    }
                    Err(e) => eprintln!("ODoH key rotation error: {}", e),
                };
            }
        });

        Ok(ODoHRotator {
            key: Arc::clone(&odoh_key),
        })
    }

    pub fn current_key(&self) -> Arc<ODoHPublicKey> {
        let key = Arc::clone(&self.key);
        Arc::clone(&key.load())
    }
}
