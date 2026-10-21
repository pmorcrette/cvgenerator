// CV Studio admin UI. Plain JavaScript, no build step: every piece of data
// comes from the REST API (/api/*), authenticated with the token stored in
// localStorage when the server requires one.

"use strict";

const TOKEN_KEY = "cvstudio.token";
const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];

const esc = (value) =>
  String(value ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

function getToken() {
  try { return localStorage.getItem(TOKEN_KEY) || ""; } catch { return ""; }
}

function setToken(token) {
  try { token ? localStorage.setItem(TOKEN_KEY, token) : localStorage.removeItem(TOKEN_KEY); } catch { /* private mode */ }
}

function toast(message, kind = "info") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  $("#toasts").append(el);
  setTimeout(() => el.remove(), 3500);
}

// Resolves once the user signed in successfully.
let loginPromise = null;

function askLogin() {
  if (loginPromise) return loginPromise;
  const dialog = $("#login-dialog");
  const form = $("#login-form");
  const error = $(".form-error", form);
  loginPromise = new Promise((resolve) => {
    form.onsubmit = async (event) => {
      event.preventDefault();
      const token = form.token.value.trim();
      const res = await fetch("/api/auth", { headers: { Authorization: `Bearer ${token}` } });
      if (res.ok) {
        setToken(token);
        dialog.close();
        loginPromise = null;
        updateLogoutButton();
        resolve();
      } else {
        error.textContent = "This token was rejected.";
        error.hidden = false;
      }
    };
  });
  dialog.addEventListener("cancel", (e) => e.preventDefault());
  dialog.showModal();
  return loginPromise;
}

function updateLogoutButton() {
  const btn = $('[data-action="logout"]');
  if (btn) btn.hidden = !getToken();
}

/** Call the REST API; asks for the token and retries on 401. */
async function api(path, { method = "GET", body, raw = false } = {}) {
  const headers = { Accept: raw ? "text/html" : "application/json" };
  const token = getToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  if (res.status === 401) {
    setToken("");
    await askLogin();
    return api(path, { method, body, raw });
  }
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try { message = (await res.json()).error || message; } catch { /* not JSON */ }
    throw new Error(message);
  }
  if (res.status === 204) return null;
  return raw ? res.text() : res.json();
}

function debounce(fn, delay) {
  let timer;
  return (...args) => { clearTimeout(timer); timer = setTimeout(() => fn(...args), delay); };
}

function initials(name) {
  return (name || "?").split(/\s+/).filter(Boolean).slice(0, 2).map((w) => w[0].toUpperCase()).join("");
}

function relativeDate(iso) {
  const seconds = (Date.now() - new Date(iso).getTime()) / 1000;
  const units = [["year", 31536000], ["month", 2592000], ["day", 86400], ["hour", 3600], ["minute", 60]];
  const fmt = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  for (const [unit, size] of units) {
    if (Math.abs(seconds) >= size) return fmt.format(-Math.round(seconds / size), unit);
  }
  return "just now";
}

let themesCache = null;
async function loadThemes(force = false) {
  if (!themesCache || force) themesCache = await api("/themes");
  return themesCache;
}

async function fillThemeSelects(selected) {
  const themes = await loadThemes();
  for (const select of $$("[data-theme-select]")) {
    select.innerHTML = themes.map((t) => `<option value="${esc(t.id)}">${esc(t.name)}</option>`).join("");
    if (selected) select.value = selected;
  }
  return themes;
}

/** Write HTML into an iframe while keeping its scroll position. */
function writePreview(iframe, html) {
  const scroll = iframe.contentWindow?.scrollY ?? 0;
  iframe.srcdoc = html;
  iframe.onload = () => iframe.contentWindow.scrollTo(0, scroll);
}

// ----- Dashboard ----------------------------------------------------------

async function initDashboard() {
  const grid = $("#cv-grid");
  const dialog = $("#new-cv-dialog");
  const form = $("#new-cv-form");

  const render = async () => {
    const [cvs, themes] = await Promise.all([api("/cvs"), loadThemes()]);
    const accent = Object.fromEntries(themes.map((t) => [t.id, t.colors.accent]));
    const names = Object.fromEntries(themes.map((t) => [t.id, t.name]));
    grid.removeAttribute("aria-busy");
    grid.innerHTML =
      cvs.map((cv) => `
        <article class="card" style="--swatch: ${esc(accent[cv.theme] || "")}">
          <div class="card-head">
            <div class="avatar">${esc(initials(cv.name))}</div>
            <div>
              <h3>${esc(cv.name || cv.id)}</h3>
              <p>${esc(cv.label || "No headline yet")}</p>
            </div>
          </div>
          <div class="meta">
            <span class="chip"><i></i>${esc(names[cv.theme] || cv.theme)}</span>
            ${cv.lang !== "en" ? `<span class="chip">${esc(cv.lang.toUpperCase())}</span>` : ""}
            <span>Updated ${esc(relativeDate(cv.updatedAt))}</span>
          </div>
          <div class="card-actions">
            <a class="btn small primary" href="/admin/cvs/${esc(cv.id)}">Edit</a>
            <a class="btn small" href="/cv/${esc(cv.id)}" target="_blank" rel="noopener">View</a>
            <button class="btn small ghost" type="button" data-duplicate="${esc(cv.id)}">Duplicate</button>
            <button class="btn small ghost danger" type="button" data-delete="${esc(cv.id)}">Delete</button>
          </div>
        </article>`).join("") +
      `<button class="card new" type="button" data-action="new-cv"><span class="plus">+</span>Create a CV</button>`;
  };

  grid.addEventListener("click", async (event) => {
    const dup = event.target.closest("[data-duplicate]");
    const del = event.target.closest("[data-delete]");
    try {
      if (dup) {
        const id = dup.dataset.duplicate;
        const newId = prompt("Id of the copy", `${id}-copy`);
        if (!newId) return;
        await api(`/cvs/${encodeURIComponent(id)}/duplicate`, { method: "POST", body: { id: newId } });
        toast("CV duplicated");
        await render();
      } else if (del) {
        const id = del.dataset.delete;
        if (!confirm(`Delete the CV "${id}"? This cannot be undone.`)) return;
        await api(`/cvs/${encodeURIComponent(id)}`, { method: "DELETE" });
        toast("CV deleted");
        await render();
      }
    } catch (err) {
      toast(err.message, "error");
    }
  });

  document.addEventListener("click", async (event) => {
    if (!event.target.closest('[data-action="new-cv"]')) return;
    await fillThemeSelects();
    $(".form-error", form).hidden = true;
    form.reset();
    dialog.showModal();
  });

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const data = new FormData(form);
    const basics = { name: data.get("name").trim() };
    if (data.get("label").trim()) basics.label = data.get("label").trim();
    let resume = { basics };
    if (data.get("example")) {
      const example = await api(`/example?lang=${encodeURIComponent(data.get("lang"))}`).catch(() => null);
      resume = example ? { ...example, basics: { ...example.basics, ...basics } } : resume;
    }
    try {
      const body = { resume, theme: data.get("theme"), lang: data.get("lang") };
      const doc = await api("/cvs", { method: "POST", body });
      location.href = `/admin/cvs/${encodeURIComponent(doc.id)}`;
    } catch (err) {
      const error = $(".form-error", form);
      error.textContent = err.message;
      error.hidden = false;
    }
  });
  $("[data-close]", form).addEventListener("click", () => dialog.close());

  // MCP connection help.
  const url = `${location.origin}/mcp`;
  $("#mcp-url").value = url;
  const config = { mcpServers: { cvgenerator: { type: "http", url } } };
  if (document.body.dataset.auth === "true") config.mcpServers.cvgenerator.headers = { Authorization: "Bearer <CV_API_TOKEN>" };
  $("#mcp-snippet").textContent = JSON.stringify(config, null, 2);

  try {
    await render();
  } catch (err) {
    grid.innerHTML = `<p class="empty">${esc(err.message)}</p>`;
  }
}

// ----- Editor -------------------------------------------------------------

async function initEditor() {
  const id = document.body.dataset.cv;
  const editor = $("#json-editor");
  const iframe = $("#preview");
  const status = $("#save-status");
  const saveBtn = $("#save-btn");
  const themeSelect = $("#theme-select");
  const layoutSelect = $("#layout-select");
  const langSelect = $("#lang-select");
  const errorBox = $("#editor-error");
  let savedText = "";
  let savedTheme = "";
  let savedLayout = "";
  let savedLang = "";

  const isDirty = () =>
    editor.value !== savedText ||
    themeSelect.value !== savedTheme ||
    layoutSelect.value !== savedLayout ||
    langSelect.value !== savedLang;

  const setStatus = (state, text) => { status.dataset.state = state; status.textContent = text; };

  const parse = () => {
    try {
      const value = JSON.parse(editor.value);
      if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error("The CV must be a JSON object");
      errorBox.hidden = true;
      return value;
    } catch (err) {
      errorBox.textContent = err.message;
      errorBox.hidden = false;
      return null;
    }
  };

  const refreshDirty = () => {
    const dirty = isDirty();
    saveBtn.disabled = !dirty;
    if (dirty) setStatus("dirty", "Unsaved changes");
    else setStatus("saved", "Saved");
    return dirty;
  };

  const updatePreview = debounce(async () => {
    const resume = parse();
    if (!resume) return;
    try {
      const body = { resume, theme: themeSelect.value, layout: layoutSelect.value || undefined, lang: langSelect.value };
      const html = await api("/preview", { method: "POST", body, raw: true });
      writePreview(iframe, html);
    } catch (err) {
      errorBox.textContent = err.message;
      errorBox.hidden = false;
    }
  }, 350);

  const save = async () => {
    const resume = parse();
    if (!resume) { setStatus("error", "Invalid JSON"); return; }
    saveBtn.disabled = true;
    setStatus("idle", "Saving…");
    try {
      // `layout: null` goes back to the theme's own layout.
      const body = { resume, theme: themeSelect.value, layout: layoutSelect.value || null, lang: langSelect.value };
      const doc = await api(`/cvs/${encodeURIComponent(id)}`, { method: "PUT", body });
      savedText = editor.value;
      savedTheme = doc.theme;
      savedLayout = doc.layout || "";
      savedLang = doc.lang;
      refreshDirty();
      toast("CV saved");
    } catch (err) {
      setStatus("error", "Save failed");
      toast(err.message, "error");
      saveBtn.disabled = false;
    }
  };

  try {
    const [doc] = await Promise.all([api(`/cvs/${encodeURIComponent(id)}`), fillThemeSelects()]);
    editor.value = savedText = JSON.stringify(doc.resume, null, 2);
    themeSelect.value = savedTheme = doc.theme;
    layoutSelect.value = savedLayout = doc.layout || "";
    langSelect.value = savedLang = doc.lang;
    refreshDirty();
    updatePreview();
  } catch (err) {
    setStatus("error", "Not found");
    editor.value = "";
    editor.disabled = true;
    toast(err.message, "error");
    return;
  }

  editor.addEventListener("input", () => { refreshDirty(); updatePreview(); });
  themeSelect.addEventListener("change", () => { refreshDirty(); updatePreview(); });
  layoutSelect.addEventListener("change", () => { refreshDirty(); updatePreview(); });
  langSelect.addEventListener("change", () => { refreshDirty(); updatePreview(); });
  saveBtn.addEventListener("click", save);

  $("#format-btn").addEventListener("click", () => {
    const value = parse();
    if (value) { editor.value = JSON.stringify(value, null, 2); refreshDirty(); }
  });

  // Jump to a section of the JSON document.
  for (const tab of $$("[data-jump]")) {
    tab.addEventListener("click", () => {
      let key = tab.dataset.jump;
      if (key === "more") key = ["volunteer", "awards", "certificates", "publications", "interests", "references"].find((k) => editor.value.includes(`"${k}"`)) || "";
      const index = key ? editor.value.indexOf(`"${key}"`) : -1;
      if (index < 0) {
        toast(key ? `No "${key}" section yet: add it to the JSON.` : "No other section yet.");
        return;
      }
      editor.focus();
      editor.setSelectionRange(index, index + key.length + 2);
      const line = editor.value.slice(0, index).split("\n").length;
      editor.scrollTop = Math.max(0, (line - 3) * parseFloat(getComputedStyle(editor).lineHeight));
    });
  }

  editor.addEventListener("keydown", (event) => {
    if (event.key === "Tab") {
      event.preventDefault();
      document.execCommand("insertText", false, "  ");
    } else if (event.key === "F" && event.shiftKey && event.altKey) {
      $("#format-btn").click();
    }
  });

  document.addEventListener("keydown", (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (!saveBtn.disabled) save();
    }
  });

  window.addEventListener("beforeunload", (event) => {
    if (isDirty()) event.preventDefault();
  });
}

// ----- Themes -------------------------------------------------------------

async function initThemes() {
  const grid = $("#theme-grid");
  const dialog = $("#theme-dialog");
  const form = $("#theme-form");
  const iframe = $("#theme-preview");
  const errorBox = $(".form-error", form);
  const deleteBtn = $('[data-action="delete-theme"]', form);
  let editing = null; // id of the custom theme being edited, null when creating

  const render = async () => {
    const themes = await loadThemes(true);
    grid.removeAttribute("aria-busy");
    grid.innerHTML = themes.map((t) => `
      <article class="card">
        <div class="swatches">
          <span style="background:${esc(t.colors.background)}"></span>
          <span style="background:${esc(t.colors.surface)}"></span>
          <span style="background:${esc(t.colors.sidebar)}"></span>
          <span class="accent" style="background:${esc(t.colors.accent)}"></span>
          <span style="background:${esc(t.colors.accentSoft)}"></span>
          <span style="background:${esc(t.colors.text)}"></span>
        </div>
        <div class="theme-sample">
          <strong>${esc(t.name)}</strong>
          <span class="muted">${esc(t.description || "Custom theme")}</span>
        </div>
        <div class="meta">
          <span class="chip">${esc(t.layout)}</span>
          <span>${t.builtin ? "Built-in" : "Custom"}</span>
        </div>
        <div class="card-actions">
          <button class="btn small ${t.builtin ? "" : "primary"}" type="button" data-edit="${esc(t.id)}">${t.builtin ? "Customize" : "Edit"}</button>
        </div>
      </article>`).join("") +
      `<button class="card new" type="button" data-action="new-theme"><span class="plus">+</span>Create a theme</button>`;
  };

  const formToTheme = () => {
    const theme = { colors: {}, fonts: {} };
    for (const el of form.elements) {
      if (!el.name) continue;
      const [group, key] = el.name.split(".");
      if (key) theme[group][key] = el.value.trim();
      else theme[group] = el.value.trim();
    }
    return theme;
  };

  const themeToForm = (theme) => {
    for (const el of form.elements) {
      if (!el.name) continue;
      const [group, key] = el.name.split(".");
      const value = key ? theme[group]?.[key] : theme[group];
      if (value === undefined) continue;
      // <input type=color> only accepts #rrggbb.
      if (el.type === "color" && !/^#[0-9a-f]{6}$/i.test(value)) continue;
      el.value = value;
    }
  };

  const updatePreview = debounce(async () => {
    try {
      const html = await api("/preview", { method: "POST", body: { themeData: { ...formToTheme(), id: "preview", name: "Preview" } }, raw: true });
      errorBox.hidden = true;
      writePreview(iframe, html);
    } catch (err) {
      errorBox.textContent = err.message;
      errorBox.hidden = false;
    }
  }, 250);

  const open = (theme, isCustom) => {
    form.reset();
    errorBox.hidden = true;
    editing = isCustom ? theme.id : null;
    const draft = structuredClone(theme);
    if (!isCustom) {
      draft.id = theme.builtin ? `${theme.id}-custom` : theme.id;
      draft.name = theme.builtin ? `${theme.name} (custom)` : theme.name;
    }
    themeToForm(draft);
    form.elements.namedItem("id").readOnly = isCustom;
    deleteBtn.hidden = !isCustom;
    $("#theme-dialog-title").textContent = isCustom ? `Edit “${theme.name}”` : "New theme";
    dialog.showModal();
    updatePreview();
  };

  document.addEventListener("click", async (event) => {
    const edit = event.target.closest("[data-edit]");
    if (edit) {
      const theme = (await loadThemes()).find((t) => t.id === edit.dataset.edit);
      open(theme, !theme.builtin);
    } else if (event.target.closest('[data-action="new-theme"]')) {
      const base = (await loadThemes())[0];
      open({ ...base, id: "my-theme", name: "My theme", description: "", builtin: false }, false);
    }
  });

  form.addEventListener("input", updatePreview);
  $("[data-close]", form).addEventListener("click", () => dialog.close());

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const theme = formToTheme();
    try {
      if (editing) await api(`/themes/${encodeURIComponent(editing)}`, { method: "PUT", body: theme });
      else await api("/themes", { method: "POST", body: theme });
      toast("Theme saved");
      dialog.close();
      await render();
    } catch (err) {
      errorBox.textContent = err.message;
      errorBox.hidden = false;
    }
  });

  deleteBtn.addEventListener("click", async () => {
    if (!editing || !confirm(`Delete the theme "${editing}"?`)) return;
    try {
      await api(`/themes/${encodeURIComponent(editing)}`, { method: "DELETE" });
      toast("Theme deleted");
      dialog.close();
      await render();
    } catch (err) {
      errorBox.textContent = err.message;
      errorBox.hidden = false;
    }
  });

  try {
    await render();
  } catch (err) {
    grid.innerHTML = `<p class="empty">${esc(err.message)}</p>`;
  }
}

// ----- Boot ---------------------------------------------------------------

document.addEventListener("click", async (event) => {
  const copy = event.target.closest("[data-copy]");
  if (copy) {
    await navigator.clipboard.writeText($(copy.dataset.copy).value);
    toast("Copied to clipboard");
  }
  if (event.target.closest('[data-action="logout"]')) {
    setToken("");
    location.reload();
  }
});

updateLogoutButton();
({ cvs: initDashboard, editor: initEditor, themes: initThemes })[document.body.dataset.page]?.();
