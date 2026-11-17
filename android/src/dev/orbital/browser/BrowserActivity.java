package dev.orbital.browser;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.SharedPreferences;
import android.graphics.Color;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.Bundle;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.webkit.CookieManager;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;

public final class BrowserActivity extends Activity {
    private static final String HOME = "https://appassets.androidplatform.net/";
    private static final int BACKGROUND = Color.rgb(16, 19, 16);
    private static final int AMBER = Color.rgb(223, 188, 114);
    private static final int MAX_TABS = 16;
    private final ArrayList<WebView> tabs = new ArrayList<>();
    private SharedPreferences storage;
    private LinearLayout chrome;
    private FrameLayout content;
    private EditText address;
    private Button tabButton;
    private WebView active;
    private View fullscreen;
    private WebChromeClient.CustomViewCallback fullscreenCallback;

    @Override public void onCreate(Bundle saved) {
        super.onCreate(saved);
        storage = getSharedPreferences("browser", MODE_PRIVATE);
        getWindow().setStatusBarColor(BACKGROUND);
        getWindow().setNavigationBarColor(BACKGROUND);

        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(BACKGROUND);
        chrome = new LinearLayout(this);
        chrome.setPadding(dp(3), 0, dp(3), 0);
        root.addView(chrome, new LinearLayout.LayoutParams(-1, dp(52)));

        Button back = addButton("<", "Back", v -> navigateBack());
        back.setMinWidth(dp(42));
        address = new EditText(this);
        address.setSingleLine(true);
        address.setTextColor(Color.WHITE);
        address.setHintTextColor(0xffa3a69e);
        address.setHint("Address or search");
        address.setTypeface(Typeface.MONOSPACE);
        address.setTextSize(14);
        address.setInputType(android.text.InputType.TYPE_CLASS_TEXT | android.text.InputType.TYPE_TEXT_VARIATION_URI);
        address.setImeOptions(EditorInfo.IME_ACTION_GO);
        address.setOnEditorActionListener((view, action, event) -> {
            if (action != EditorInfo.IME_ACTION_GO) return false;
            String input = address.getText().toString().trim();
            if (!input.isEmpty()) {
                String normalized = input.matches("(?i)^https?://.*") ? input
                        : input.contains(".") && !input.matches(".*\\s.*") ? "https://" + input
                        : "https://duckduckgo.com/?q=" + Uri.encode(input);
                if (allowed(Uri.parse(normalized))) active.loadUrl(normalized);
            }
            ((InputMethodManager) getSystemService(INPUT_METHOD_SERVICE)).hideSoftInputFromWindow(address.getWindowToken(), 0);
            address.clearFocus();
            return true;
        });
        chrome.addView(address, new LinearLayout.LayoutParams(0, -1, 1));
        tabButton = addButton("1", "Tabs", v -> showTabs());
        addButton("...", "Browser menu", v -> showMenu());

        content = new FrameLayout(this);
        root.addView(content, new LinearLayout.LayoutParams(-1, 0, 1));
        setContentView(root);
        newTab();
    }

    private int dp(int px) {
        return Math.round(px * getResources().getDisplayMetrics().density);
    }

    private Button addButton(String text, String label, View.OnClickListener handler) {
        Button button = new Button(this);
        button.setText(text);
        button.setContentDescription(label);
        button.setTextColor(AMBER);
        button.setTypeface(Typeface.MONOSPACE);
        button.setAllCaps(false);
        button.setMinWidth(dp(42));
        button.setPadding(dp(6), 0, dp(6), 0);
        button.setOnClickListener(handler);
        chrome.addView(button, new LinearLayout.LayoutParams(-2, -1));
        return button;
    }

    private static boolean allowed(Uri url) {
        return "https".equalsIgnoreCase(url.getScheme()) && url.getHost() != null;
    }

    private void newTab() {
        if (tabs.size() >= MAX_TABS) return;
        WebView view = new WebView(this);
        view.setBackgroundColor(BACKGROUND);
        view.getSettings().setJavaScriptEnabled(true);
        view.getSettings().setDomStorageEnabled(true);
        view.getSettings().setAllowFileAccess(false);
        view.getSettings().setAllowContentAccess(false);
        view.getSettings().setSupportMultipleWindows(false);
        CookieManager.getInstance().setAcceptCookie(true);
        CookieManager.getInstance().setAcceptThirdPartyCookies(view, false);
        view.setWebViewClient(new WebViewClient() {
            @Override public boolean shouldOverrideUrlLoading(WebView web, WebResourceRequest request) {
                return request.isForMainFrame() && !allowed(request.getUrl());
            }

            @Override public WebResourceResponse shouldInterceptRequest(WebView web, WebResourceRequest request) {
                Uri url = request.getUrl();
                if (!"appassets.androidplatform.net".equals(url.getHost())) return null;
                if (!allowed(url)) return emptyResponse();
                String path = url.getPath();
                if ("/".equals(path)) return asset("start.html", "text/html", "utf-8");
                if ("/terminal.ttf".equals(path)) return asset("terminal.ttf", "font/ttf", null);
                return emptyResponse();
            }

            @Override public void onPageFinished(WebView web, String url) {
                if (web == active && !address.hasFocus()) address.setText(HOME.equals(url) ? "" : url);
                if (HOME.equals(url)) {
                    web.evaluateJavascript("window.setBookmarks?.(" + entries("bookmarks") + ");window.setFleet?.("
                            + fleet() + ")", null);
                } else if (allowed(Uri.parse(url))) {
                    remember("history", web.getTitle(), url, 100);
                }
                CookieManager.getInstance().flush();
            }
        });
        view.setWebChromeClient(new WebChromeClient() {
            @Override public void onShowCustomView(View video, CustomViewCallback callback) {
                if (view != active || fullscreen != null) {
                    callback.onCustomViewHidden();
                    return;
                }
                fullscreen = video;
                fullscreenCallback = callback;
                chrome.setVisibility(View.GONE);
                active.setVisibility(View.GONE);
                content.addView(video, new FrameLayout.LayoutParams(-1, -1));
                getWindow().getDecorView().setSystemUiVisibility(5894);
            }

            @Override public void onHideCustomView() { closeFullscreen(); }
        });
        tabs.add(view);
        content.addView(view, new FrameLayout.LayoutParams(-1, -1));
        select(view);
        view.loadUrl(HOME);
    }

    private WebResourceResponse asset(String name, String mime, String encoding) {
        try {
            return new WebResourceResponse(mime, encoding, 200, "OK",
                    Collections.singletonMap("Content-Security-Policy",
                        "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; font-src 'self'; form-action https://duckduckgo.com; base-uri 'none'"),
                    getAssets().open(name));
        } catch (IOException error) { return emptyResponse(); }
    }

    private static WebResourceResponse emptyResponse() {
        return new WebResourceResponse("text/plain", "utf-8", 404, "Not Found",
                Collections.emptyMap(), new java.io.ByteArrayInputStream(new byte[0]));
    }

    private JSONArray fleet() {
        JSONArray ids = new JSONArray();
        for (int i = 0; i < tabs.size(); ++i) ids.put(i);
        return ids;
    }

    private void select(WebView view) {
        closeFullscreen();
        active = view;
        for (WebView tab : tabs) {
            boolean selected = tab == active;
            tab.setVisibility(selected ? View.VISIBLE : View.GONE);
            if (selected) tab.onResume(); else tab.onPause();
            if (HOME.equals(tab.getUrl())) {
                tab.evaluateJavascript("window.setStartActive?.(" + selected + ");window.setFleet?.(" + fleet() + ")", null);
            }
        }
        tabButton.setText(Integer.toString(tabs.size()));
        address.setText(HOME.equals(view.getUrl()) ? "" : view.getUrl());
    }

    private void closeCurrent() {
        closeFullscreen();
        if (tabs.size() == 1) {
            active.loadUrl(HOME);
            return;
        }
        int index = tabs.indexOf(active);
        tabs.remove(index);
        content.removeView(active);
        active.destroy();
        select(tabs.get(Math.max(0, index - 1)));
    }

    private void closeFullscreen() {
        if (fullscreen == null) return;
        content.removeView(fullscreen);
        fullscreen = null;
        chrome.setVisibility(View.VISIBLE);
        if (active != null) active.setVisibility(View.VISIBLE);
        getWindow().getDecorView().setSystemUiVisibility(0);
        WebChromeClient.CustomViewCallback callback = fullscreenCallback;
        fullscreenCallback = null;
        if (callback != null) callback.onCustomViewHidden();
    }

    private void navigateBack() {
        if (fullscreen != null) closeFullscreen();
        else if (active.canGoBack()) active.goBack();
        else if (tabs.size() > 1) closeCurrent();
        else if (!HOME.equals(active.getUrl())) active.loadUrl(HOME);
        else finish();
    }

    @Override public void onBackPressed() { navigateBack(); }

    private void showTabs() {
        String[] labels = new String[tabs.size()];
        for (int i = 0; i < labels.length; ++i) {
            String title = tabs.get(i).getTitle();
            labels[i] = title == null ? "New tab" : title;
        }
        new AlertDialog.Builder(this).setTitle("Tabs").setItems(labels, (dialog, index) -> select(tabs.get(index)))
                .setPositiveButton("New tab", (dialog, button) -> newTab())
                .setNegativeButton("Close current", (dialog, button) -> closeCurrent()).show();
    }

    private void showMenu() {
        String[] options = {"Start page", "Bookmarks", "Bookmark current page", "History", "Reload"};
        new AlertDialog.Builder(this).setTitle("Browser").setItems(options, (dialog, index) -> {
            switch (index) {
                case 0: active.loadUrl(HOME); break;
                case 1: showEntries("bookmarks", "Bookmarks"); break;
                case 2:
                    String url = active.getUrl();
                    if (url != null && !HOME.equals(url) && allowed(Uri.parse(url))) remember("bookmarks", active.getTitle(), url, 40);
                    break;
                case 3: showEntries("history", "History"); break;
                case 4: active.reload(); break;
                default: break;
            }
        }).show();
    }

    private JSONArray entries(String key) {
        try { return new JSONArray(storage.getString(key, "[]")); }
        catch (JSONException error) { return new JSONArray(); }
    }

    private void remember(String key, String title, String url, int limit) {
        try {
            JSONArray previous = entries(key);
            JSONArray next = new JSONArray().put(new JSONObject().put("name", title == null ? url : title).put("url", url));
            for (int i = 0; i < previous.length() && next.length() < limit; ++i) {
                JSONObject entry = previous.optJSONObject(i);
                if (entry != null && !url.equals(entry.optString("url"))) next.put(entry);
            }
            storage.edit().putString(key, next.toString()).apply();
        } catch (JSONException ignored) { }
    }

    private void showEntries(String key, String title) {
        JSONArray data = entries(key);
        String[] names = new String[data.length()];
        for (int i = 0; i < names.length; ++i) names[i] = data.optJSONObject(i).optString("name");
        new AlertDialog.Builder(this).setTitle(title)
                .setItems(names, (dialog, index) -> active.loadUrl(data.optJSONObject(index).optString("url")))
                .setNegativeButton("Close", null).show();
    }

    @Override protected void onPause() {
        if (active != null) active.onPause();
        CookieManager.getInstance().flush();
        super.onPause();
    }

    @Override protected void onResume() {
        super.onResume();
        if (active != null) active.onResume();
    }

    @Override protected void onDestroy() {
        closeFullscreen();
        for (WebView tab : tabs) {
            if (tab.getParent() instanceof ViewGroup) ((ViewGroup) tab.getParent()).removeView(tab);
            tab.destroy();
        }
        tabs.clear();
        super.onDestroy();
    }
}
