package dk.maarse.app;

import android.Manifest;
import android.annotation.SuppressLint;
import android.app.Activity;
import android.app.AlertDialog;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothGatt;
import android.bluetooth.BluetoothGattCallback;
import android.bluetooth.BluetoothGattCharacteristic;
import android.bluetooth.BluetoothGattDescriptor;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothProfile;
import android.bluetooth.le.BluetoothLeScanner;
import android.bluetooth.le.ScanCallback;
import android.bluetooth.le.ScanFilter;
import android.bluetooth.le.ScanResult;
import android.bluetooth.le.ScanSettings;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelUuid;
import android.util.Base64;
import android.view.Window;
import android.webkit.GeolocationPermissions;
import android.webkit.JavascriptInterface;
import android.webkit.PermissionRequest;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.ArrayAdapter;

import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.UUID;

/**
 * Må:rse for Android: the web client (assets/index.html) in a WebView plus a
 * Bluetooth bridge to the node (Nordic UART Service), because WebView has no Web
 * Bluetooth. The web client uses the bridge via window.MaarseAndroid and gets data
 * back through window.__maarseBle.
 *
 * The page is served from https://app.maarse.local/ (intercepted here), so it is a
 * secure context: microphone and DecompressionStream work.
 */
public class MainActivity extends Activity {
    static final String ORIGIN = "https://app.maarse.local/";
    static final UUID NUS = UUID.fromString("6e400001-b5a3-f393-e0a9-e50e24dcca9e");
    static final UUID NUS_RX = UUID.fromString("6e400002-b5a3-f393-e0a9-e50e24dcca9e"); // we write
    static final UUID NUS_TX = UUID.fromString("6e400003-b5a3-f393-e0a9-e50e24dcca9e"); // the node notifies
    static final UUID CCCD = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb");
    static final int REQ_BLE = 1, REQ_MIC = 2, REQ_GEO = 3;

    WebView web;
    final Handler main = new Handler(Looper.getMainLooper());
    BluetoothAdapter adapter;
    BluetoothGatt gatt;
    BluetoothGattCharacteristic rx;
    final ArrayDeque<byte[]> outQueue = new ArrayDeque<>();
    boolean writing = false;
    int chunk = 20;
    PermissionRequest pendingMic;
    // Location for the range test page (phone GPS per received beacon).
    GeolocationPermissions.Callback pendingGeo;
    String pendingGeoOrigin;

    @SuppressLint("SetJavaScriptEnabled")
    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        Window w = getWindow();
        w.setStatusBarColor(Color.parseColor("#151d1a"));
        w.setNavigationBarColor(Color.parseColor("#151d1a"));
        BluetoothManager bm = (BluetoothManager) getSystemService(BLUETOOTH_SERVICE);
        adapter = bm == null ? null : bm.getAdapter();

        web = new WebView(this);
        WebSettings s = web.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setMediaPlaybackRequiresUserGesture(false);
        s.setAllowFileAccess(false);
        s.setGeolocationEnabled(true);
        // ws:// to a maarse-host (e.g. over VPN) from the secure internal page.
        s.setMixedContentMode(WebSettings.MIXED_CONTENT_ALWAYS_ALLOW);
        web.addJavascriptInterface(new Bridge(), "MaarseAndroid");
        web.setWebViewClient(new WebViewClient() {
            @Override
            public WebResourceResponse shouldInterceptRequest(WebView v, WebResourceRequest r) {
                String u = r.getUrl().toString();
                if (!u.startsWith(ORIGIN)) return null;
                String path = r.getUrl().getPath();
                if (path == null || path.equals("/") || path.isEmpty()) path = "/index.html";
                try {
                    InputStream in = getAssets().open("web" + path);
                    return new WebResourceResponse(mime(path), "utf-8", in);
                } catch (IOException e) {
                    return new WebResourceResponse("text/plain", "utf-8", 404, "Not Found", null, null);
                }
            }
        });
        web.setWebChromeClient(new WebChromeClient() {
            @Override
            public boolean onConsoleMessage(android.webkit.ConsoleMessage m) {
                android.util.Log.i("maarse", m.messageLevel() + " " + m.message() + " (" + m.sourceId() + ":" + m.lineNumber() + ")");
                return true;
            }

            @Override
            public void onGeolocationPermissionsShowPrompt(String origin, GeolocationPermissions.Callback cb) {
                // Only our own page asks; the user still answers Android's prompt.
                if (!origin.startsWith(ORIGIN.substring(0, ORIGIN.length() - 1))) { cb.invoke(origin, false, false); return; }
                main.post(() -> {
                    if (checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED) {
                        cb.invoke(origin, true, false);
                    } else {
                        pendingGeo = cb;
                        pendingGeoOrigin = origin;
                        requestPermissions(new String[]{Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION}, REQ_GEO);
                    }
                });
            }

            @Override
            public void onPermissionRequest(PermissionRequest req) {
                main.post(() -> {
                    if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
                        req.grant(req.getResources());
                    } else {
                        pendingMic = req;
                        requestPermissions(new String[]{Manifest.permission.RECORD_AUDIO}, REQ_MIC);
                    }
                });
            }
        });
        setContentView(web);
        web.loadUrl(ORIGIN + "index.html");
    }

    static String mime(String p) {
        if (p.endsWith(".html")) return "text/html";
        if (p.endsWith(".js")) return "text/javascript";
        if (p.endsWith(".svg")) return "image/svg+xml";
        if (p.endsWith(".png")) return "image/png";
        if (p.endsWith(".webmanifest") || p.endsWith(".json")) return "application/json";
        return "application/octet-stream";
    }

    @Override
    public void onBackPressed() {
        if (web.canGoBack()) web.goBack(); else super.onBackPressed();
    }

    @Override
    protected void onDestroy() {
        closeGatt();
        super.onDestroy();
    }

    // ---------- JavaScript ----------

    void js(String call) {
        main.post(() -> web.evaluateJavascript("window.__maarseBle && window.__maarseBle." + call, null));
    }

    static String q(String s) {
        return "'" + s.replace("\\", "\\\\").replace("'", "\\'") + "'";
    }

    class Bridge {
        @JavascriptInterface
        public void connect() {
            main.post(MainActivity.this::startConnect);
        }

        @JavascriptInterface
        public void write(String b64) {
            byte[] data = Base64.decode(b64, Base64.NO_WRAP);
            synchronized (outQueue) {
                for (int i = 0; i < data.length; i += chunk) {
                    byte[] part = new byte[Math.min(chunk, data.length - i)];
                    System.arraycopy(data, i, part, 0, part.length);
                    outQueue.add(part);
                }
            }
            pump();
        }

        /** Hands a text file (e.g. a range test CSV) to another app: Drive, mail, Files … */
        @JavascriptInterface
        public void shareText(String name, String text) {
            main.post(() -> {
                Intent send = new Intent(Intent.ACTION_SEND);
                send.setType("text/csv");
                send.putExtra(Intent.EXTRA_SUBJECT, name);
                send.putExtra(Intent.EXTRA_TITLE, name);
                send.putExtra(Intent.EXTRA_TEXT, text);
                startActivity(Intent.createChooser(send, name));
            });
        }

        @JavascriptInterface
        public void disconnect() {
            main.post(MainActivity.this::closeGatt);
        }
    }

    // ---------- permissions ----------

    String[] blePerms() {
        if (Build.VERSION.SDK_INT >= 31)
            return new String[]{Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT};
        return new String[]{Manifest.permission.ACCESS_FINE_LOCATION};
    }

    boolean hasBlePerms() {
        for (String p : blePerms())
            if (checkSelfPermission(p) != PackageManager.PERMISSION_GRANTED) return false;
        return true;
    }

    @Override
    public void onRequestPermissionsResult(int code, String[] perms, int[] res) {
        boolean ok = res.length > 0;
        for (int r : res) ok &= r == PackageManager.PERMISSION_GRANTED;
        if (code == REQ_BLE) {
            if (ok) startConnect(); else js("onError(" + q("Bluetooth permission was denied") + ")");
        } else if (code == REQ_GEO && pendingGeo != null) {
            // Coarse alone is enough to answer; the page then gets a rough position.
            boolean any = false;
            for (int r : res) any |= r == PackageManager.PERMISSION_GRANTED;
            pendingGeo.invoke(pendingGeoOrigin, any, false);
            pendingGeo = null;
        } else if (code == REQ_MIC && pendingMic != null) {
            if (ok) pendingMic.grant(pendingMic.getResources()); else pendingMic.deny();
            pendingMic = null;
        }
    }

    // ---------- scanning and choosing a node ----------

    @SuppressLint("MissingPermission")
    void startConnect() {
        if (adapter == null) { js("onError(" + q("The phone has no Bluetooth") + ")"); return; }
        if (!hasBlePerms()) { requestPermissions(blePerms(), REQ_BLE); return; }
        if (!adapter.isEnabled()) { js("onError(" + q("Turn on Bluetooth and try again") + ")"); return; }
        BluetoothLeScanner sc = adapter.getBluetoothLeScanner();
        if (sc == null) { js("onError(" + q("Bluetooth is not ready") + ")"); return; }

        final List<BluetoothDevice> found = new ArrayList<>();
        final ArrayAdapter<String> names = new ArrayAdapter<>(this, android.R.layout.select_dialog_item);
        final ScanCallback cb = new ScanCallback() {
            @Override
            public void onScanResult(int type, ScanResult r) {
                BluetoothDevice d = r.getDevice();
                for (BluetoothDevice f : found) if (f.getAddress().equals(d.getAddress())) return;
                found.add(d);
                String n = d.getName();
                if (n == null && r.getScanRecord() != null) n = r.getScanRecord().getDeviceName();
                names.add((n != null ? n : "Unknown node") + "  ·  " + r.getRssi() + " dBm");
                names.notifyDataSetChanged();
            }
        };
        ScanFilter f = new ScanFilter.Builder().setServiceUuid(new ParcelUuid(NUS)).build();
        ScanSettings set = new ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build();
        sc.startScan(Collections.singletonList(f), set, cb);
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
                .setTitle("Choose a Må:rse node")
                .setAdapter(names, (dlg, which) -> { sc.stopScan(cb); connectTo(found.get(which)); })
                .setNegativeButton("Cancel", (dlg, which) -> { sc.stopScan(cb); js("onError(" + q("Cancelled") + ")"); })
                .setOnCancelListener(dlg -> { sc.stopScan(cb); js("onError(" + q("Cancelled") + ")"); })
                .show();
    }

    // ---------- GATT ----------

    @SuppressLint("MissingPermission")
    void connectTo(BluetoothDevice d) {
        closeGatt();
        final String name = d.getName() != null ? d.getName() : "unknown";
        gatt = d.connectGatt(this, false, new BluetoothGattCallback() {
            @Override
            public void onConnectionStateChange(BluetoothGatt g, int status, int state) {
                if (state == BluetoothProfile.STATE_CONNECTED) {
                    g.requestMtu(247);
                } else if (state == BluetoothProfile.STATE_DISCONNECTED) {
                    g.close();
                    if (g == gatt) { gatt = null; rx = null; js("onClose()"); }
                }
            }

            @Override
            public void onMtuChanged(BluetoothGatt g, int mtu, int status) {
                if (status == BluetoothGatt.GATT_SUCCESS) chunk = Math.max(20, mtu - 3);
                g.discoverServices();
            }

            @Override
            public void onServicesDiscovered(BluetoothGatt g, int status) {
                android.bluetooth.BluetoothGattService s = g.getService(NUS);
                if (s == null) { js("onError(" + q("The node has no Nordic UART") + ")"); g.disconnect(); return; }
                rx = s.getCharacteristic(NUS_RX);
                BluetoothGattCharacteristic tx = s.getCharacteristic(NUS_TX);
                g.setCharacteristicNotification(tx, true);
                BluetoothGattDescriptor cccd = tx.getDescriptor(CCCD);
                cccd.setValue(BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE);
                g.writeDescriptor(cccd);
            }

            @Override
            public void onDescriptorWrite(BluetoothGatt g, BluetoothGattDescriptor d, int status) {
                js("onOpen(" + q(name) + ")");
            }

            @Override
            public void onCharacteristicChanged(BluetoothGatt g, BluetoothGattCharacteristic c) {
                byte[] v = c.getValue();
                if (v != null && v.length > 0) js("onData(" + q(Base64.encodeToString(v, Base64.NO_WRAP)) + ")");
            }

            @Override
            public void onCharacteristicWrite(BluetoothGatt g, BluetoothGattCharacteristic c, int status) {
                synchronized (outQueue) { writing = false; }
                pump();
            }
        }, BluetoothDevice.TRANSPORT_LE);
    }

    /** Writes the next chunk; one write at a time (Android requires it). */
    @SuppressLint("MissingPermission")
    void pump() {
        byte[] next;
        synchronized (outQueue) {
            if (writing || rx == null || gatt == null || outQueue.isEmpty()) return;
            next = outQueue.poll();
            writing = true;
        }
        rx.setWriteType(BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT);
        rx.setValue(next);
        if (!gatt.writeCharacteristic(rx)) {
            synchronized (outQueue) { writing = false; outQueue.addFirst(next); }
            main.postDelayed(this::pump, 20);
        }
    }

    @SuppressLint("MissingPermission")
    void closeGatt() {
        synchronized (outQueue) { outQueue.clear(); writing = false; }
        if (gatt != null) { gatt.disconnect(); gatt.close(); gatt = null; rx = null; js("onClose()"); }
    }
}
