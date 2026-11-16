package com.kobaproduction.vpnclient;

import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.KeyStore;
import java.util.Arrays;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

final class CatalogSeal {
    private static final String PROVIDER = "AndroidKeyStore";
    private static final String ALIAS = "vpn-client.catalog.aes.v1";
    private static final String TRANSFORMATION = "AES/GCM/NoPadding";
    private static final byte[] CONTEXT = "vpn-client.catalog.android.v1"
        .getBytes(StandardCharsets.US_ASCII);
    private static final int IV_BYTES = 12;
    private static final int TAG_BITS = 128;
    private static final byte VERSION = 1;

    static byte[] transform(byte[] input, boolean encrypt) {
        try {
            return encrypt ? encrypt(input) : decrypt(input);
        } catch (GeneralSecurityException | IOException failure) {
            // JNI maps a null result to a typed storage error; never log catalog contents.
            return null;
        } finally {
            Arrays.fill(input, (byte) 0);
        }
    }

    private static SecretKey key(boolean create) throws GeneralSecurityException, IOException {
        KeyStore store = KeyStore.getInstance(PROVIDER);
        store.load(null);
        if (store.getKey(ALIAS, null) instanceof SecretKey existing) return existing;
        if (!create) throw new GeneralSecurityException("Catalog key unavailable");
        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER);
        generator.init(new KeyGenParameterSpec.Builder(
            ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
            .setKeySize(256)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setRandomizedEncryptionRequired(true)
            .build());
        return generator.generateKey();
    }

    private static byte[] encrypt(byte[] input) throws GeneralSecurityException, IOException {
        Cipher cipher = Cipher.getInstance(TRANSFORMATION);
        cipher.init(Cipher.ENCRYPT_MODE, key(true));
        byte[] iv = cipher.getIV();
        if (iv.length != IV_BYTES) throw new GeneralSecurityException("Unexpected IV length");
        cipher.updateAAD(CONTEXT);
        byte[] sealed = cipher.doFinal(input);
        byte[] envelope = new byte[1 + IV_BYTES + sealed.length];
        envelope[0] = VERSION;
        System.arraycopy(iv, 0, envelope, 1, IV_BYTES);
        System.arraycopy(sealed, 0, envelope, 1 + IV_BYTES, sealed.length);
        return envelope;
    }

    private static byte[] decrypt(byte[] input) throws GeneralSecurityException, IOException {
        if (input.length < 1 + IV_BYTES + TAG_BITS / 8 || input[0] != VERSION) {
            throw new GeneralSecurityException("Invalid catalog envelope");
        }
        Cipher cipher = Cipher.getInstance(TRANSFORMATION);
        cipher.init(Cipher.DECRYPT_MODE, key(false), new GCMParameterSpec(TAG_BITS, input, 1, IV_BYTES));
        cipher.updateAAD(CONTEXT);
        return cipher.doFinal(input, 1 + IV_BYTES, input.length - 1 - IV_BYTES);
    }

    private CatalogSeal() {}
}
