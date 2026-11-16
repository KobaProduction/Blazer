//! Android adapters for the application ports.

mod tunnel;

use std::sync::Arc;

use crate::client_store::CatalogProtection;
use jni::objects::{Global, JByteArray, JClass, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use vpn_application::client::ClientError;

pub use tunnel::AndroidEngine;

pub struct AndroidPlatform {
    vm: JavaVM,
    hooks: Global<JClass<'static>>,
}

impl AndroidPlatform {
    pub fn new(env: &mut Env<'_>, hooks: &JClass<'_>) -> jni::errors::Result<Self> {
        Ok(Self {
            vm: env.get_java_vm()?,
            hooks: env.new_global_ref(hooks)?,
        })
    }

    fn seal(&self, input: &[u8], encrypt: bool) -> jni::errors::Result<Vec<u8>> {
        self.vm.attach_current_thread(|env| {
            let input = env.byte_array_from_slice(input)?;
            let output = env
                .call_static_method(
                    &*self.hooks,
                    jni_str!("sealCatalog"),
                    jni_sig!("([BZ)[B"),
                    &[JValue::Object(&input), JValue::Bool(encrypt)],
                )?
                .l()?;
            let output = JByteArray::cast_local(env, output)?;
            env.convert_byte_array(&output)
        })
    }

    fn boolean_hook(&self, method: &jni::strings::JNIStr) -> jni::errors::Result<bool> {
        self.vm.attach_current_thread(|env| {
            env.call_static_method(&*self.hooks, method, jni_sig!("()Z"), &[])?
                .z()
        })
    }

    fn open_tunnel(&self) -> jni::errors::Result<i32> {
        self.vm.attach_current_thread(|env| {
            env.call_static_method(&*self.hooks, jni_str!("openTunnel"), jni_sig!("()I"), &[])?
                .i()
        })
    }

    fn start_tunnel(&self, config: &str) -> jni::errors::Result<bool> {
        self.vm.attach_current_thread(|env| {
            let config = env.new_string(config)?;
            env.call_static_method(
                &*self.hooks,
                jni_str!("startTunnel"),
                jni_sig!("(Ljava/lang/String;)Z"),
                &[JValue::Object(&config)],
            )?
            .z()
        })
    }
}

pub struct AndroidProtection(pub Arc<AndroidPlatform>);

impl CatalogProtection for AndroidProtection {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ClientError> {
        self.0
            .seal(plaintext, true)
            .map_err(|_| ClientError::StorageUnavailable)
    }

    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, ClientError> {
        self.0
            .seal(ciphertext, false)
            .map_err(|_| ClientError::StorageCorrupt)
    }
}
