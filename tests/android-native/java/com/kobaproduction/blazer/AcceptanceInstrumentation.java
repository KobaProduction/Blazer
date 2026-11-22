package com.kobaproduction.blazer;

import android.app.Instrumentation;
import android.content.Context;
import android.net.VpnService;
import android.os.Bundle;
import com.blazecore.BlazeCoreNative;
import java.io.File;
import java.io.InputStream;
import java.lang.reflect.Method;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import org.json.JSONObject;

/** Real ClientVpnService/JNI acceptance, packaged exclusively in local debug APKs. */
public final class AcceptanceInstrumentation extends Instrumentation {
    private Bundle arguments;

    @Override public void onCreate(Bundle values) {
        arguments = values;
        start();
    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        File directory = null;
        try {
            Context context = getTargetContext();
            require(BlazeCoreNative.available(), "trusted BlazeCore library unavailable");
            require(BlazeCoreNative.selectorControlAvailable(), "selector-control ABI unavailable");
            require(VpnService.prepare(context) == null, "VPN consent must be granted on the isolated AVD");
            directory = new File(context.getNoBackupFilesDir(), "acceptance-" + System.nanoTime());
            Method open = NativeSession.class.getDeclaredMethod("open", String.class, boolean.class);
            open.setAccessible(true);
            require((Boolean) open.invoke(null, directory.getAbsolutePath(), true), "isolated client session failed");
            String configuration = new String(android.util.Base64.decode(arguments.getString("config"), android.util.Base64.DEFAULT), StandardCharsets.UTF_8);
            long request = state("import", configuration).getLong("request");
            awaitImported(request);
            startVpn(context);
            long first = ClientVpnService.tunnelHandle();
            require(first > 0, "core returned an invalid generation handle");
            awaitRoute("A");
            try (Socket held = stream()) {
                require(exchange(held).equals("A"), "held connection did not use A");
                stage("fail_a");
                awaitRoute("B");
                require(exchange(held).equals("A"), "existing TUN stream changed outbound");
                stage("all_dead");
                awaitRoute("C");
                stage("recover_a");
                awaitRoute("A");
                require(exchange(held).equals("A"), "recovery changed an established stream");
            }
            stopVpn();
            require(BlazeCoreNative.selectOutbound(first, "blazer-graph-group-0", "blazer-graph-outbound-1") == -2,
                    "stopped generation accepted a selector mutation");
            startVpn(context);
            long second = ClientVpnService.tunnelHandle();
            require(second > first, "restart did not allocate a fresh core generation");
            require(BlazeCoreNative.selectOutbound(first, "blazer-graph-group-0", "blazer-graph-outbound-2") == -2,
                    "old generation mutated the restarted session");
            awaitRoute("A");
            stopVpn();
            require(state("shutdown", "").optBoolean("stopped"), "isolated client shutdown failed");
            result.putString("stream", "PASS leastPing A->B->fallback C->A, held flow, actual TUN/JNI and stale generation\n");
            finish(0, result);
        } catch (Throwable failure) {
            try { state("disconnect", ""); } catch (Throwable ignored) {}
            try { state("shutdown", ""); } catch (Throwable ignored) {}
            result.putString("stream", "FAIL Android native acceptance: " + failure + "\n");
            finish(1, result);
        } finally {
            if (directory != null) deleteOwned(directory);
        }
    }

    private static JSONObject state(String operation, String payload) throws Exception {
        return new JSONObject(AcceptanceBridge.execute(operation, payload));
    }

    private static void awaitImported(long request) throws Exception {
        long deadline = System.nanoTime() + 15_000_000_000L;
        while (System.nanoTime() < deadline) {
            JSONObject status = state("status", "");
            if (status.optLong("completed") >= request) {
                require(status.isNull("error") && status.optInt("profiles") == 1, "graph import failed: " + status);
                return;
            }
            Thread.sleep(30);
        }
        throw new AssertionError("graph import timed out");
    }

    private static void startVpn(Context context) throws Exception {
        ClientVpnService.request(context);
        long deadline = System.nanoTime() + 20_000_000_000L;
        while (System.nanoTime() < deadline) {
            JSONObject status = state("status", "");
            if ("Tun".equals(status.optString("active")) && ClientVpnService.tunnelRunning()) return;
            if (!status.isNull("error")) throw new AssertionError("TUN start failed: " + status);
            Thread.sleep(40);
        }
        throw new AssertionError("actual VPN did not become ready");
    }

    private static void stopVpn() throws Exception {
        state("disconnect", "");
        long deadline = System.nanoTime() + 10_000_000_000L;
        while (System.nanoTime() < deadline) {
            JSONObject status = state("status", "");
            if (!BlazeCoreNative.isRunning() && status.isNull("active") && !status.optBoolean("cleanup")) return;
            Thread.sleep(30);
        }
        throw new AssertionError("VPN or observer did not clean up");
    }

    private static Socket stream() throws Exception {
        Socket socket = new Socket();
        socket.connect(new InetSocketAddress("198.51.100.213", 443), 1500);
        socket.setSoTimeout(1500);
        return socket;
    }

    private static String exchange(Socket socket) throws Exception {
        socket.getOutputStream().write(new byte[] {'P', 'I', 'N', 'G'});
        byte[] reply = new byte[6];
        InputStream input = socket.getInputStream();
        int received = 0;
        while (received < reply.length) {
            int count = input.read(reply, received, reply.length - received);
            if (count < 0) throw new AssertionError("TUN stream closed");
            received += count;
        }
        return new String(reply, 0, 1, StandardCharsets.US_ASCII);
    }

    private static void awaitRoute(String expected) throws Exception {
        long deadline = System.nanoTime() + 12_000_000_000L;
        String last = "unreachable";
        while (System.nanoTime() < deadline) {
            try (Socket socket = stream()) {
                last = exchange(socket);
                if (expected.equals(last)) return;
            } catch (java.io.IOException transientFailure) {}
            Thread.sleep(80);
        }
        throw new AssertionError("expected exit " + expected + ", observed " + last);
    }

    private void stage(String value) {
        Bundle update = new Bundle();
        update.putString("stage", value);
        sendStatus(0, update);
    }

    private static void require(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static void deleteOwned(File file) {
        File[] children = file.listFiles();
        if (children != null) for (File child : children) deleteOwned(child);
        file.delete();
    }
}
