// 瞬移终端：按交互设计稿实施的真实客户端。
// 原型 DOM/CSS 原样复用（src/proto/），本文件把演示数据替换为真实后端：
// 项目=本地目录（懒加载文件树+只读预览），终端=xterm 真会话（本地 PTY/SSH/瞬移协议），指令=真实发送。
import { invoke } from "@tauri-apps/api/core";
import { makeDesktop } from "./desktop.js";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "./proto/style.css";
import "./proto/projects.css";
import "./proto/commands.css";
import "./proto/overrides.css";
import "./host-dialog.css";
import "./proto/icons.js";
import hljs from "./vendor/highlight.js";

const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const icon = (name) => '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + (window.prototypeIcons[name] || window.prototypeIcons.file) + "</svg>";
document.querySelectorAll("[data-icon]").forEach((el) => (el.innerHTML = icon(el.dataset.icon)));

const inTauri = "__TAURI_INTERNALS__" in window;
// Windows 用系统原生标题栏，隐藏自绘标题栏（红绿灯是 macOS 专属）；侧栏开关走 Ctrl+B
if (/Windows/i.test(navigator.userAgent)) document.body.classList.add("platform-win");

const notify = (message) => {
  $("#toast").textContent = message;
  $("#toast").hidden = false;
  clearTimeout(notify.timer);
  notify.timer = setTimeout(() => ($("#toast").hidden = true), 3200);
};

// ---------- 持久化 ----------
const prefsKey = "shunyi.workspace.v1";
const settingsKey = "unirc.settings";
const hostsKey = "unirc.hosts";
const commandsKey = "unirc.snippets";

const settings = loadJson(settingsKey, { serverUrl: "", token: "" });
let hosts = loadJson(hostsKey, []); // 保存的主机（含凭据，仅本机）
const devices = []; // 免账号版仅展示用户保存的主机，不公布全网设备。
const temporaryPasswords = new Map(); // 仅在连接前短暂保留，绝不持久化。
let commandRecords = loadJson(commandsKey, [
  { id: "cmd-pwd", name: "当前目录", command: "pwd" },
  { id: "cmd-disk", name: "磁盘空间", command: "df -h" },
  { id: "cmd-git", name: "Git 工作区状态", command: "git status --short" },
]);

function loadJson(key, fallback) {
  try {
    const v = JSON.parse(localStorage.getItem(key) || "null");
    return v ?? fallback;
  } catch {
    return fallback;
  }
}
function saveJson(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
    return true;
  } catch {
    notify("数据未能保存到本机存储。");
    return false;
  }
}

// ---------- 工作区 / 项目状态 ----------
let prefs = loadJson(prefsKey, {});
if (!prefs || typeof prefs !== "object" || Array.isArray(prefs)) prefs = {};
let activeWorkspace = prefs.activeWorkspace === "remote" ? "remote" : "project";
let remoteExpanded = prefs.remoteExpanded !== false;
let projectsSectionExpanded = prefs.projectsSectionExpanded !== false;
let projectExpanded = true;
const remoteWorkspace = { id: null, name: "远程主机", visited: false };
let sequence = 1;
let activeProjectId = null;
let activeId = null;
let commandPanelOpen = true;
let editingCommandId = null;
let deletedCommand = null;
let selectedHosts = new Set();

const projects = []; // Local folders, directory caches, and their own terminal/file tabs.
const tabs = []; // {id,type:"terminal"|"file",projectId,...}

function currentOwnerId() { return activeWorkspace === "remote" ? null : activeProjectId; }
function currentWorkspace() { return activeWorkspace === "remote" ? remoteWorkspace : currentProject(); }
function tabWorkspace(t) { return t?.projectId === null ? remoteWorkspace : tabProject(t); }
function currentProject() {
  return projects.find((p) => p.id === activeProjectId);
}
function workspaceTabs(projectId = currentOwnerId()) {
  return tabs.filter((t) => t.projectId === projectId);
}
function activeTab() {
  return workspaceTabs().find((t) => t.id === activeId);
}
function tabProject(t) {
  return projects.find((p) => p.id === t?.projectId);
}

// ---------- 主机池（保存的主机 + 在线设备；显示范围为全局选择，不属于项目） ----------
function hostPool() {
  const saved = hosts.map((h) => ({
    id: h.id,
    name: h.name,
    address: h.proto === "unirc" ? (h.deviceId || "-") : `${h.host}:${Number(h.port) || 22}`,
    user: h.username || "",
    group: h.group || "默认",
    protocol: h.proto === "unirc" ? "unirc" : "ssh",
    kind: "host",
  }));
  const live = devices.map((d) => ({
    id: "dev:" + d.device_id,
    name: d.name || d.device_id,
    address: d.device_id,
    user: "—",
    group: "自有设备",
    protocol: "unirc",
    kind: "device",
  }));
  const seen = new Set();
  return [...saved, ...live].filter((h) => (seen.has(h.id) ? false : (seen.add(h.id), true)));
}
function poolEntry(id) {
  return hostPool().find((h) => h.id === id);
}
function allVisibleHosts() {
  const pool = hostPool();
  const vis = prefs.visibleHostIds;
  return vis ? pool.filter((h) => vis.includes(h.id)) : pool;
}
function hostIsOpen(id) {
  return tabs.some((t) => t.type === "terminal" && t.hostId === id && t.projectId === null && t.session);
}

// ---------- 通用 ----------
function rememberWorkspace() {
  const workspace = currentWorkspace(), t = activeTab();
  if (workspace && t) {
    workspace.lastActiveTabId = t.id;
    if (t.type === "terminal") workspace.lastTerminalId = t.id;
  }
}
function rememberProject() {
  rememberWorkspace();
  if (activeWorkspace === "project" && currentProject()) currentProject().fileFilter = $("#file-filter").value;
}
function saveWorkspace() {
  rememberProject();
  prefs = {
    activeProjectId, activeWorkspace, remoteExpanded, projectsSectionExpanded,
    filters: { search: $("#host-search").value, protocol: $("#protocol-filter").value || "all", group: $("#group-filter").value || "all" },
    visibleHostIds: prefs.visibleHostIds ?? null,
    width: $("#sidebar").getBoundingClientRect().width || 300,
    projects: Object.fromEntries(projects.map((p) => [p.id, { name: p.name, rootPath: p.rootPath, expandedDirs: [...p.expandedDirs], rootExpanded: p.rootExpanded, fileFilter: p.fileFilter || "" }]))
  };
  saveJson(prefsKey, prefs);
}

// ---------- Projects and global remote connections ----------
function renderProjects() {
  const content = $("#project-content"), isProject = activeWorkspace === "project";
  content.remove();
  $("#project-list").innerHTML = projects.map((p) => {
    const selected = isProject && p.id === activeProjectId, open = selected && projectExpanded;
    return '<section class="project-group' + (open ? ' expanded' : '') + '" data-project-group="' + esc(p.id) + '"><div class="project-row"><button class="project-select' + (selected ? ' selected' : '') + '" data-project="' + esc(p.id) + '" aria-expanded="' + open + '" aria-controls="project-body-' + esc(p.id) + '" title="' + esc(p.name + ' · ' + p.rootPath) + '"><span class="project-chevron">' + icon(open ? 'chevron-down' : 'chevron-right') + '</span><span class="project-folder">' + icon(open ? 'folder-open' : 'folder') + '</span><span class="project-name">' + esc(p.name) + '</span><span class="project-session-count" data-project-count="' + esc(p.id) + '"></span></button></div><div id="project-body-' + esc(p.id) + '" class="project-body"' + (open ? '' : ' hidden') + '></div></section>';
  }).join("");
  const body = document.getElementById("project-body-" + activeProjectId);
  (body || $("#sidebar")).appendChild(content);
  content.hidden = !isProject || !projectExpanded;
  $("#project-list").hidden = !projectsSectionExpanded;
  $("#project-count").textContent = projects.length;
  $("#collapse-projects").setAttribute("aria-expanded", String(projectsSectionExpanded));
  $("#collapse-projects").title = projectsSectionExpanded ? "收起项目列表" : "展开项目列表";
  $("#collapse-projects").classList.toggle("selected", isProject);
  $("#collapse-projects .sidebar-group-chevron").innerHTML = icon(projectsSectionExpanded ? "chevron-down" : "chevron-right");
  const remoteOpen = !isProject && remoteExpanded;
  $("#remote-pane").hidden = !remoteOpen;
  $("#sidebar").classList.toggle("remote-active", remoteOpen);
  $("#remote-workspace-button").classList.toggle("selected", !isProject);
  $("#remote-workspace-button").setAttribute("aria-expanded", String(remoteOpen));
  $("#remote-workspace-button").title = remoteOpen ? "收起远程主机列表" : "展开远程主机列表";
  $("#remote-workspace-button .remote-chevron").innerHTML = icon(remoteOpen ? "chevron-down" : "chevron-right");
  $("#project-context-label").textContent = currentWorkspace()?.name || "项目";
  $("#workspace-kind-label").textContent = isProject ? "项目工作区" : "全局连接";
  $("#open-terminal").title = "打开" + (currentWorkspace()?.name || "当前工作区") + "的终端";
  updateProjectBadges();
}
function updateProjectBadges() {
  document.querySelectorAll("[data-project-count]").forEach((el) => {
    const count = workspaceTabs(el.dataset.projectCount).filter((t) => t.type === "terminal" && !t.disposed).length;
    el.textContent = count ? String(count) : "";
    el.title = count + " 个已打开的终端";
    el.setAttribute("aria-label", el.title);
  });
}
function loadProjectContext() { $("#file-filter").value = currentProject()?.fileFilter || ""; }
function loadRemoteContext() {
  const filters = prefs.filters || {};
  $("#host-search").value = filters.search || "";
  $("#protocol-filter").value = filters.protocol || "all";
  if (!$("#protocol-filter").value) $("#protocol-filter").value = "all";
  syncGroupFilter(hostPool());
  $("#group-filter").value = filters.group || "all";
  if (!$("#group-filter").value) $("#group-filter").value = "all";
}
function selectProject(id, toggle = false, { restore = true, reveal = true } = {}) {
  if (!projects.some((p) => p.id === id)) return;
  const changed = activeWorkspace !== "project" || id !== activeProjectId;
  rememberProject();
  projectExpanded = !changed && toggle ? !projectExpanded : true;
  if (reveal) projectsSectionExpanded = true;
  activeWorkspace = "project"; activeProjectId = id;
  loadProjectContext(); renderProjects(); renderTree(); renderHosts(); saveWorkspace();
  $("#workspace").classList.remove("sidebar-hidden");
  if (restore && (changed || !activeTab())) restoreWorkspace();
}
function selectRemoteWorkspace({ restore = true, reveal = true, toggle = false } = {}) {
  const changed = activeWorkspace !== "remote";
  if (!changed && toggle) { remoteExpanded = !remoteExpanded; renderProjects(); saveWorkspace(); return; }
  rememberProject(); activeWorkspace = "remote";
  if (reveal) remoteExpanded = true;
  renderProjects(); renderHosts(); saveWorkspace();
  $("#workspace").classList.remove("sidebar-hidden");
  if (restore && (changed || !activeTab())) restoreWorkspace();
}
function restoreWorkspace() {
  const workspace = currentWorkspace(), owned = workspaceTabs();
  const terminal = owned.find((t) => t.id === workspace.lastTerminalId && t.type === "terminal") || owned.find((t) => t.type === "terminal");
  const next = terminal || owned.find((t) => t.id === workspace.lastActiveTabId) || owned[0];
  const firstVisit = !workspace.visited; workspace.visited = true;
  if (next) return activateTab(next.id);
  activeId = null;
  if (firstVisit && activeWorkspace === "project") return newTerminal();
  renderTabs(); renderMain();
}
function chooseRemoteHost() {
  selectRemoteWorkspace();
  if (!hostPool().length) return openHostDialog();
  if (!allVisibleHosts().length) return showHostPicker();
  $("#host-search").focus();
  notify("从列表中选择主机，打开远程终端。");
}
function openWorkspaceTerminal() {
  const workspace = currentWorkspace(), owned = workspaceTabs();
  const terminal = owned.find((t) => t.type === "terminal" && t.id === workspace?.lastTerminalId) || owned.find((t) => t.type === "terminal");
  if (terminal) activateTab(terminal.id);
  else if (activeWorkspace === "remote") chooseRemoteHost();
  else newTerminal();
}
function makeProject(id, name, rootPath, saved = {}) {
  return { id, name, rootPath, expandedDirs: new Set(Array.isArray(saved.expandedDirs) ? saved.expandedDirs : []), rootExpanded: saved.rootExpanded !== false, fileFilter: saved.fileFilter || "", dirCache: new Map(), dirLoading: new Map(), dirErrors: new Map(), lastActiveTabId: null, lastTerminalId: null, visited: false };
}
async function addProject(rootPath, customName) {
  const norm = String(rootPath).replace(/\/+$/, "") || "/";
  const existing = projects.find((p) => p.rootPath === norm);
  if (existing) { selectProject(existing.id); notify("项目「" + existing.name + "」已在列表中"); return; }
  rememberProject();
  const name = customName || norm.split("/").filter(Boolean).pop() || norm;
  const p = makeProject("project-" + Date.now().toString(36) + "-" + sequence++, name, norm);
  projects.push(p); selectProject(p.id); notify("已添加项目「" + name + "」");
}
function renameProject(id, newName) {
  const p = projects.find((proj) => proj.id === id);
  if (!p || !newName.trim()) return;
  p.name = newName.trim();
  renderProjects(); saveWorkspace();
  notify("已重命名为「" + p.name + "」");
}
function removeProject(id) {
  const index = projects.findIndex((p) => p.id === id);
  if (index < 0) return;
  const p = projects[index];
  tabs.filter((t) => t.projectId === id).forEach((t) => closeTab(t.id));
  projects.splice(index, 1);
  if (activeProjectId === id) {
    if (projects.length) selectProject(projects[Math.min(index, projects.length - 1)].id);
    else selectRemoteWorkspace();
  } else renderProjects();
  saveWorkspace();
  notify("已移除项目「" + p.name + "」");
}

// ---------- 标签页 ----------
function renderTabs() {
  $("#tabbar").setAttribute("aria-label", (currentWorkspace()?.name || "") + (activeWorkspace === "remote" ? "的终端" : "的终端和文件"));
  $("#tabbar").innerHTML =
    workspaceTabs()
      .map((t) =>
        '<div class="tab' + (t.type === "file" && !t.pinned ? " preview" : "") + '" role="tab" tabindex="' + (t.id === activeId ? "0" : "-1") + '" aria-selected="' + (t.id === activeId) + '" data-tab="' + esc(t.id) + '" title="' + esc(t.type === "file" ? t.rel + (t.pinned ? " · 已固定" : " · 临时预览，双击固定") : (tabWorkspace(t)?.name || "") + " · " + t.title) + '">' +
        (t.type === "file" ? fileSymbol(t.name) : '<span class="tab-icon">' + icon(t.type === "desktop" ? "monitor" : "terminal") + "</span>") +
        '<span class="tab-title">' + esc(t.title) + '</span><button class="tab-close" data-close-tab="' + esc(t.id) + '" aria-label="关闭 ' + esc(t.title) + '">' + icon("close") + "</button></div>"
      )
      .join("") +
    '<button class="tab-add" data-action="' + (activeWorkspace === "remote" ? "choose-remote" : "new-terminal") + '" title="新建会话" aria-label="' + (activeWorkspace === "remote" ? "新建远程连接" : "新建本地终端") + '">' + icon("plus") + "</button>";
}
function activateTab(id, { focusTerminal = true } = {}) {
  const t = tabs.find((tab) => tab.id === id);
  if (!t) return;
  if (t.projectId === null) { if (activeWorkspace !== "remote") selectRemoteWorkspace({ restore: false }); }
  else if (activeWorkspace !== "project" || t.projectId !== activeProjectId) selectProject(t.projectId, false, { restore: false });
  activeId = id;
  const p = currentWorkspace();
  p.visited = true;
  p.lastActiveTabId = id;
  if (t.type === "terminal") p.lastTerminalId = id;
  renderTabs();
  renderMain({ focusTerminal });
  updateTreeSelection();
  renderHosts();
  updateProjectBadges();
}
function closeTab(id) {
  const index = tabs.findIndex((t) => t.id === id);
  if (index < 0) return;
  const before = workspaceTabs(tabs[index].projectId);
  const position = before.findIndex((t) => t.id === id);
  const [closed] = tabs.splice(index, 1);
  closed.disposed = true;
  closed.closed = true;
  if (closed.session) closeBackendSession(closed.kind, closed.session);
  closed.desktop?.dispose();
  closed._ro?.disconnect();
  closed.term?.dispose();
  closed.container?.remove();
  const remaining = workspaceTabs(closed.projectId);
  const p = tabWorkspace(closed);
  if (p) {
    if (p.lastTerminalId === id) p.lastTerminalId = remaining.find((t) => t.type === "terminal")?.id || null;
    if (p.lastActiveTabId === id) p.lastActiveTabId = remaining[Math.min(position, remaining.length - 1)]?.id || null;
  }
  if (activeId === id) {
    activeId = null;
    const next = remaining[Math.min(position, remaining.length - 1)];
    if (next) {
      activateTab(next.id);
      return;
    }
  }
  renderTabs();
  renderMain();
  renderTree();
  renderHosts();
  updateProjectBadges();
}

// ---------- 真实终端会话 ----------
function makeTerm() {
  const term = new Terminal({
    fontFamily: '"SF Mono", Menlo, Monaco, Consolas, monospace',
    fontSize: 13,
    cursorBlink: true,
    scrollback: 10000,
    screenReaderMode: true,
    theme: { background: "#141519", foreground: "#d6d9de", cursor: "#4f8cff", selectionBackground: "#2a4a7a" },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  return { term, fit };
}

function createTerminalTab({ kind, title, hostId = null, hostEntry = null, dir }) {
  const id = "term-" + Date.now().toString(36) + "-" + sequence++;
  const container = document.createElement("div");
  container.className = "term-container not-current";
  container.dataset.tab = id;
  $("#term-stage").appendChild(container);
  const { term, fit } = makeTerm();
  term.open(container);
  const tab = {
    id,
    type: "terminal",
    projectId: kind === "local" ? activeProjectId : null,
    kind,
    title,
    hostId,
    hostEntry: hostEntry ? { ...hostEntry } : null,
    dir: dir || "",
    session: null,
    closed: false,
    disposed: false,
    phase: "connecting",
    term,
    fit,
    container,
  };
  tabs.push(tab);
  term.onData((d) => {
    if (tab.session) sendInput(tab, d).catch(toastErr);
  });
  term.onResize(() => resizeSession(tab));
  const ro = new ResizeObserver(() => {
    if (activeTab() !== tab || $("#term-ui").hidden || tab.disposed) return;
    try {
      fit.fit();
    } catch {}
  });
  ro.observe(container);
  tab._ro = ro;
  // 建立真实会话
  bootSession(tab);
  activateTab(id);
  return tab;
}

const earlyTerminalEvents = new Map();
const retiredSessions = new Set();
function closeBackendSession(kind, session) {
  retiredSessions.add(session);
  earlyTerminalEvents.delete(session);
  if (retiredSessions.size > 256) retiredSessions.delete(retiredSessions.values().next().value);
  return invoke((kind === "local" ? "pty" : kind) + "_close", { session }).catch(() => {});
}
function resizeSession(tab) {
  if (!tab.session || tab.disposed || tab.phase !== "connected") return;
  invoke((tab.kind === "local" ? "pty" : tab.kind) + "_resize", {
    session: tab.session, cols: tab.term.cols, rows: tab.term.rows,
  }).catch(() => {});
}
function deliverTerminalEvent(tab, event) {
  if (tab.disposed) return;
  if (event.type === "data") {
    const bin = atob(event.b64);
    tab.term.write(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
  } else {
    tab.phase = "closed";
    tab.closed = true;
    if (tab.session) closeBackendSession(tab.kind, tab.session);
    tab.session = null;
    tab.term.write("\r\n\x1b[33m─ 会话已关闭" + (event.reason ? "：" + event.reason : "") + " ─\x1b[0m\r\n");
    if (activeTab() === tab) renderMain();
    renderHosts();
  }
}
function routeTerminalEvent(event) {
  const tab = tabs.find((t) => t.type === "terminal" && t.session === event.session);
  if (tab) return deliverTerminalEvent(tab, event);
  // The backend may emit the first prompt before its open invocation resolves.
  if (retiredSessions.has(event.session) || !tabs.some((t) => t.phase === "connecting")) return;
  let queue = earlyTerminalEvents.get(event.session);
  if (!queue) {
    if (earlyTerminalEvents.size >= 32) return;
    queue = { events: [], size: 0 };
    earlyTerminalEvents.set(event.session, queue);
  }
  if (queue.size < 2 * 1024 * 1024 || event.type === "closed") {
    queue.events.push(event);
    queue.size += event.b64?.length || 0;
  }
}

async function bootSession(tab) {
  if (!inTauri) {
    tab.phase = "error";
    tab.term.writeln("\x1b[33m浏览器预览模式：终端需要在 Tauri 窗口中使用。\x1b[0m");
    return;
  }
  try {
    await terminalListenersReady;
    if (tab.disposed) return;
    let session;
    if (tab.kind === "local") {
      session = await invoke("pty_open", { cols: tab.term.cols || 80, rows: tab.term.rows || 24, cwd: tab.dir || null });
    } else if (tab.kind === "ssh") {
      const h = tab.hostEntry;
      session = await invoke("ssh_connect", {
        host: h.host,
        port: Number(h.port) || 22,
        username: h.username || "root",
        authType: h.authType || "password",
        password: h.password || "",
        keyPath: h.keyPath || "",
        cols: tab.term.cols || 80,
        rows: tab.term.rows || 24,
      });
    } else {
      const d = tab.hostEntry;
      const temporaryPassword = temporaryPasswords.get(tab.hostId) || "";
      temporaryPasswords.delete(tab.hostId);
      session = await invoke("unirc_connect", { request: {
        server: settings.serverUrl,
        token: settings.token,
        deviceId: d.deviceId,
        authType: d.deviceAuth || "certificate",
        certificatePath: d.certificatePath || "",
        temporaryPassword,
        cols: tab.term.cols || 80,
        rows: tab.term.rows || 24,
      } });
    }
    if (tab.disposed) { await closeBackendSession(tab.kind, session); return; }
    tab.session = session;
    tab.phase = "connected";
    const pending = earlyTerminalEvents.get(session);
    earlyTerminalEvents.delete(session);
    for (const event of pending?.events || []) deliverTerminalEvent(tab, event);
    resizeSession(tab);
    if (activeTab() === tab) renderMain();
    renderHosts();
  } catch (e) {
    if (tab.disposed) return;
    tab.phase = "error";
    tab.term.writeln("\x1b[31m连接失败：" + (e?.message || e) + "\x1b[0m");
    tab.term.writeln("点击下方重新连接可重试。");
    if (activeTab() === tab) renderMain();
  } finally {
    if (!tabs.some((t) => t.phase === "connecting")) earlyTerminalEvents.clear();
  }
}

async function sendInput(tab, data) {
  if (!inTauri || !tab.session || tab.phase !== "connected" || tab.disposed) throw new Error("请先打开可用的终端。");
  await invoke((tab.kind === "local" ? "pty" : tab.kind) + "_write", { session: tab.session, data });
}
function retryTerminal(t = activeTab()) {
  if (t?.type !== "terminal" || t.disposed || !tabs.includes(t) || !["error", "closed"].includes(t.phase)) return;
  if (t.hostId) {
    const latest = hosts.find((host) => host.id === t.hostId);
    if (!latest) return notify("该主机已删除，请重新添加后连接。");
    t.hostEntry = { ...latest };
    t.kind = latest.proto;
  }
  if (t.kind === "unirc" && t.hostEntry?.deviceAuth === "temporary" && !temporaryPasswords.has(t.hostId)) {
    requestTemporaryPassword(t.hostId, () => {
      if (t.disposed || !tabs.includes(t)) return temporaryPasswords.delete(t.hostId);
      retryTerminal(t);
    });
    return;
  }
  t.closed = false;
  t.phase = "connecting";
  t.term.writeln("\r\n正在重新连接…");
  bootSession(t);
  renderMain();
}

function newTerminal(dir = null) {
  if (activeWorkspace === "remote") return chooseRemoteHost();
  const p = currentProject();
  if (!p) return;
  const nextDir = dir || p.rootPath;
  let count = 1;
  let title = "本地终端";
  while (workspaceTabs().some((t) => t.type === "terminal" && t.title === title)) title = "本地终端 " + ++count;
  return createTerminalTab({ kind: "local", title, dir: nextDir });
}

function connectHost(poolId) {
  const entry = poolEntry(poolId);
  if (!entry) return;
  if (activeWorkspace !== "remote") selectRemoteWorkspace({ restore: false });
  const existing = tabs.find((t) => t.type === "terminal" && t.hostId === poolId && t.projectId === null && !t.disposed);
  if (existing && ["connecting", "connected"].includes(existing.phase)) return activateTab(existing.id);
  if (existing) closeTab(existing.id);
  if (entry.protocol === "unirc" && !settings.serverUrl) {
    openSettings("连接自有设备前，请先在设置里填写瞬移服务器地址");
    return;
  }
  const hostEntry =
    entry.kind === "device"
      ? { deviceId: entry.address }
      : hosts.find((h) => h.id === entry.id);
  if (!hostEntry) return;
  if (entry.protocol === "unirc" && hostEntry.deviceAuth === "temporary" && !temporaryPasswords.has(entry.id)) {
    requestTemporaryPassword(entry.id, () => connectHost(entry.id));
    return;
  }
  const tab = createTerminalTab({
    kind: entry.protocol === "unirc" ? "unirc" : "ssh",
    title: entry.name,
    hostId: entry.id,
    hostEntry,
    dir: "~",
  });
  notify("正在连接「" + entry.name + "」…");
  return tab;
}

function openDesktopHost(poolId, control = false) {
  if (!inTauri) return notify("远程桌面需要在瞬移客户端中使用。");
  const entry = poolEntry(poolId);
  const host = hosts.find((h) => h.id === entry?.id);
  if (!host || entry.protocol !== "unirc") return;
  if (!settings.serverUrl) return openSettings("请先填写瞬移服务器地址");
  const existing = tabs.find((t) => t.type === "desktop" && t.hostId === poolId && !t.disposed);
  if (existing) return activateTab(existing.id);
  if (host.deviceAuth === "temporary" && !temporaryPasswords.has(entry.id)) return requestTemporaryPassword(entry.id, () => openDesktopHost(entry.id, control));
  selectRemoteWorkspace({ restore: false });
  const id = "desktop-" + Date.now().toString(36) + "-" + sequence++;
  const desktop = makeDesktop({ title: entry.name, control, close: () => closeTab(id), notify });
  const tab = { id, type: "desktop", kind: "desktop", projectId: null, title: entry.name + " · 桌面", hostId: entry.id, desktop, container: desktop.container, disposed: false };
  tabs.push(tab);
  $("#content").appendChild(desktop.container);
  activateTab(id, { focusTerminal: false });
  const password = temporaryPasswords.get(entry.id) || "";
  temporaryPasswords.delete(entry.id);
  desktop.connect({ server: settings.serverUrl, token: settings.token, deviceId: host.deviceId, authType: host.deviceAuth || "certificate", certificatePath: host.certificatePath || "", temporaryPassword: password, control, display: 0 });
}

// ---------- 主区渲染 ----------
function sessionAddress(t) {
  if (t.kind === "local") return "本机 · " + (t.dir || "~");
  const h = t.hostEntry || {};
  return t.kind === "ssh" ? "SSH · " + (h.host || "") + ":" + (Number(h.port) || 22) : "瞬移协议 · " + (h.deviceId || "");
}
function renderMain({ focusTerminal = false } = {}) {
  const t = activeTab();
  tabs.filter((tab) => tab.type === "desktop").forEach((tab) => { if (tab !== t) tab.desktop.hide(); else tab.container.hidden = false; });
  $("#open-terminal").classList.toggle("active", t?.type === "terminal");
  syncCommandPanel();
  const termUi = $("#term-ui");
  const fileUi = $("#file-ui");
  const empty = $("#empty-main");
  if (!t) {
    termUi.hidden = true;
    fileUi.hidden = true;
    empty.hidden = false;
    empty.innerHTML = activeWorkspace === "remote"
      ? '<div class="empty-main"><span>' + icon("server") + '</span><h2>选择一台主机，打开远程终端</h2><p>远程连接集中保留在这里。<br>从左侧主机列表选择要连接的设备。</p><button class="primary-button" data-action="choose-remote">选择远程主机</button></div>'
      : '<div class="empty-main"><span>' + icon("terminal") + '</span><h2>' + esc(currentProject()?.name || "项目") + ' · 暂无打开的会话</h2><p>从右上角打开项目终端，或点击下方新建。<br>其他项目与远程连接的会话继续保留。</p><button class="primary-button" data-action="new-terminal">新建本地终端</button><button class="secondary-button" data-action="open-folder">添加项目</button></div>';
    return;
  }
  empty.hidden = true;
  if (t.type === "desktop") { termUi.hidden = true; fileUi.hidden = true; return; }
  if (t.type === "file") {
    termUi.hidden = true;
    fileUi.hidden = false;
    renderFileTab(t);
    return;
  }
  fileUi.hidden = true;
  termUi.hidden = false;
  $("#session-project").innerHTML = icon(t.projectId === null ? "server" : "folder") + esc(tabWorkspace(t)?.name || "");
  $("#session-address").textContent = sessionAddress(t);
  $("#term-status").innerHTML =
    icon(t.kind === "local" ? "terminal" : "connection") +
    '<span class="status-' + (t.phase === "connected" ? "connected" : t.phase === "connecting" ? "pending" : "error") + '">' + (t.phase === "connected" ? (t.kind === "local" ? "本地会话" : "会话已打开") : t.phase === "connecting" ? "正在连接…" : t.phase === "closed" ? "会话已关闭" : "连接失败") + '</span>' + (["closed", "error"].includes(t.phase) ? '<button class="text-button" data-action="retry-terminal">重新连接</button>' : '') + '<span>' + esc(t.dir || "") + '</span>';
  for (const el of $("#term-stage").children) el.classList.toggle("not-current", el.dataset.tab !== t.id);
  requestAnimationFrame(() => {
    if (activeTab() !== t || t.disposed || $("#term-ui").hidden || document.querySelector("dialog[open]")) return;
    try {
      t.fit.fit();
      if (focusTerminal) t.term.focus();
    } catch {}
  });
}

// ---------- 本地文件树（懒加载真实目录） ----------
const languageMap = { ts: "typescript", js: "javascript", mjs: "javascript", cjs: "javascript", jsx: "javascript", tsx: "typescript", c: "c", h: "c", cpp: "cpp", cs: "csharp", rb: "ruby", php: "php", kt: "kotlin", dockerfile: "dockerfile", rs: "rust", py: "python", java: "java", swift: "swift", go: "go", json: "json", yaml: "yaml", yml: "yaml", toml: "ini", sh: "bash", sql: "sql", css: "css", md: "markdown", html: "xml", xml: "xml" };
function fileSymbol(name) {
  const ext = (name.split(".").pop() || "").toLowerCase();
  const label = { ts: "TS", js: "JS", rs: "RS", py: "PY", json: "{}", yaml: "Y", md: "M", swift: "SW", go: "GO", sql: "SQL", css: "#", sh: "$" }[ext];
  return '<span class="file-symbol symbol-' + esc(ext) + '">' + (label ? esc(label) : icon("file")) + "</span>";
}

async function ensureDir(abs, p = currentProject()) {
  if (!p) return [];
  if (p.dirCache.has(abs)) return p.dirCache.get(abs);
  if (p.dirLoading.has(abs)) return p.dirLoading.get(abs);
  const pending = invoke("fs_list", { dir: abs }).then((entries) => {
    p.dirCache.set(abs, entries); p.dirErrors.delete(abs); return entries;
  }).catch((e) => { p.dirErrors.set(abs, String(e?.message || e)); return []; })
    .finally(() => p.dirLoading.delete(abs));
  p.dirLoading.set(abs, pending);
  return pending;
}
function renderTree() {
  const p = currentProject();
  if (!p || activeWorkspace !== "project") return;
  const focusedPath = document.activeElement?.closest("#file-tree [data-path]")?.dataset.path;
  const filter = ($("#file-filter").value || "").trim().toLowerCase(), active = activeTab(), rows = [];
  $("#root-name").textContent = p.name;
  $("#root-toggle").title = p.rootPath;
  $("#root-toggle").setAttribute("aria-expanded", String(p.rootExpanded));
  $("#root-toggle [data-icon]").innerHTML = icon(p.rootExpanded ? "chevron-down" : "chevron-right");
  $("#file-tree").hidden = !p.rootExpanded;
  $("#source-note").textContent = filter ? "筛选已加载的目录和文件" : p.rootPath;
  $("#source-note").title = p.rootPath;
  if (!p.rootExpanded) return;
  const matches = (entry) => !filter || entry.name.toLowerCase().includes(filter) || (entry.is_dir && (p.dirCache.get(entry.path) || []).some(matches));
  const loadForTree = (path) => {
    if (!p.dirLoading.has(path) && !p.dirErrors.has(path)) ensureDir(path, p).then(() => {
      if (currentProject() === p && activeWorkspace === "project") renderTree();
    });
  };
  const renderDir = (entries, level) => {
    for (const entry of entries.filter(matches)) {
      const open = filter ? p.dirCache.has(entry.path) : p.expandedDirs.has(entry.path);
      const selected = active?.type === "file" && active.path === entry.path && !entry.is_dir;
      rows.push('<div role="treeitem" tabindex="0" class="tree-row' + (selected ? ' selected' : '') + '" style="padding-left:' + (11 + level * 16) + 'px" data-path="' + esc(entry.path) + '" data-directory="' + entry.is_dir + '" title="' + esc(entry.path) + '" aria-level="' + (level + 1) + '"' + (entry.is_dir ? ' aria-expanded="' + Boolean(open) + '"' : ' aria-selected="' + selected + '"') + '><span class="chevron">' + (entry.is_dir ? icon(open ? 'chevron-down' : 'chevron-right') : '') + '</span>' + (entry.is_dir ? '<span class="file-symbol">' + icon(open ? 'folder-open' : 'folder') + '</span>' : fileSymbol(entry.name)) + '<span class="tree-label">' + esc(entry.name) + '</span></div>');
      if (entry.is_dir && open) {
        if (p.dirCache.has(entry.path)) renderDir(p.dirCache.get(entry.path), level + 1);
        else if (!filter) { loadForTree(entry.path); rows.push('<div class="tree-empty">' + esc(p.dirErrors.get(entry.path) || '正在读取目录…') + '</div>'); }
      }
    }
  };
  if (p.dirCache.has(p.rootPath)) renderDir(p.dirCache.get(p.rootPath), 0);
  else loadForTree(p.rootPath);
  const empty = p.dirErrors.get(p.rootPath) || (p.dirLoading.has(p.rootPath) ? '正在读取目录…' : filter ? '没有匹配的文件。展开目录后可继续筛选。' : '目录为空。');
  $("#file-tree").innerHTML = rows.join('') || '<div class="tree-empty">' + esc(empty) + '</div>';
  if (focusedPath) [...$("#file-tree").querySelectorAll("[data-path]")].find((row) => row.dataset.path === focusedPath)?.focus();
}
async function toggleDirectory(path) {
  const p = currentProject();
  if (p.expandedDirs.has(path)) p.expandedDirs.delete(path);
  else { p.expandedDirs.add(path); p.dirErrors.delete(path); await ensureDir(path, p); }
  if (currentProject() === p && activeWorkspace === 'project') { saveWorkspace(); renderTree(); }
}
function updateTreeSelection() {
  const t = activeTab();
  document.querySelectorAll('#file-tree [data-directory="false"]').forEach((row) => {
    const selected = t?.type === 'file' && t.path === row.dataset.path;
    row.classList.toggle('selected', selected); row.setAttribute('aria-selected', String(selected));
  });
}

// ---------- 文件预览 ----------
function openFile(path, pinned = false) {
  const p = currentProject();
  if (!p || activeWorkspace !== "project") return;
  let t = workspaceTabs().find((tab) => tab.type === "file" && tab.path === path);
  if (!t) {
    const previewIndex = tabs.findIndex((tab) => tab.type === "file" && !tab.pinned && tab.projectId === activeProjectId);
    t = {
      id: "file-" + Date.now().toString(36) + "-" + sequence++,
      type: "file",
      projectId: activeProjectId,
      path,
      name: path.split("/").pop(),
      title: path.split("/").pop(),
      rel: path.startsWith(p.rootPath + "/") ? path.slice(p.rootPath.length + 1) : path,
      rootName: p.name,
      pinned,
      loaded: null,
      loading: false,
      wrap: false,
      markdown: false,
    };
    if (previewIndex >= 0) tabs.splice(previewIndex, 1, t);
    else tabs.push(t);
  } else if (pinned) t.pinned = true;
  activateTab(t.id);
}

async function loadFileTab(t) {
  if (t.loaded || t.loading) return;
  t.loading = true;
  try {
    const res = await invoke("fs_read", { path: t.path });
    t.loaded = { type: "text", content: res.content, lines: res.lines };
  } catch (e) {
    t.loaded = { type: "error", message: String(e?.message || e) };
  }
  t.loading = false;
  if (activeId === t.id) renderMain();
}

function languageName(name) {
  const map = { typescript: "TypeScript", javascript: "JavaScript", rust: "Rust", python: "Python", json: "JSON", yaml: "YAML", ini: "TOML / INI", bash: "Shell", markdown: "Markdown", xml: "HTML / XML", sql: "SQL", css: "CSS", go: "Go", java: "Java", swift: "Swift" };
  return map[name] || "纯文本";
}

function markdownHtml(content){
  return content.split(/\n\n+/).map((p)=>{
    if(p.startsWith("# "))return"<h1>"+esc(p.slice(2))+"</h1>";
    if(p.startsWith("## "))return"<h2>"+esc(p.slice(3))+"</h2>";
    if(p.split("\n").every((line)=>line.startsWith("- ")))return"<ul>"+p.split("\n").map((line)=>"<li>"+esc(line.slice(2))+"</li>").join("")+"</ul>";
    return"<p>"+esc(p).replace(/\n/g,"<br>")+"</p>";
  }).join("");
}

function renderFileTab(t) {
  if (!t.loaded) loadFileTab(t);
  let body = "";
  if (!t.loaded) body = '<div class="file-error" role="status">正在读取文件…</div>';
  else if (t.loaded.type === "error") body = '<div class="file-error"><strong>暂时无法预览这个文件</strong><p>' + esc(t.loaded.message) + "</p></div>";
  else if (t.markdown) body = '<article class="markdown-preview">' + markdownHtml(t.loaded.content) + "</article>";
  else {
    const content = t.loaded.content;
    const numbers = Array.from({ length: content.split("\n").length }, (_, i) => i + 1).join("\n");
    const lang = languageMap[t.name.split(".").pop().toLowerCase()];
    let highlighted = esc(content);
    try { if (lang && hljs.getLanguage(lang)) highlighted = hljs.highlight(content, { language: lang, ignoreIllegals: true }).value; } catch {}
    body = '<div class="code-scroller' + (t.wrap ? " wrap" : "") + '"><div class="gutter" aria-hidden="true">' + numbers + '</div><pre><code class="hljs">' + highlighted + "</code></pre></div>";
  }
  $("#file-ui").innerHTML =
    '<div class="file-toolbar"><div class="breadcrumb"><span>' + esc(t.rootName) + "</span>" + icon("chevron-right") + '<span title="' + esc(t.rel) + '">' + esc(t.rel) + '</span></div><div class="toolbar-right"><span class="readonly-tag">' + icon("lock") + "只读</span>" +
    (t.rel.endsWith(".md") ? '<button class="text-button" data-action="markdown">' + (t.markdown ? "查看源码" : "阅读预览") + "</button>" : "") +
    '<button class="icon-button' + (t.wrap ? " active" : "") + '" data-action="wrap" title="自动换行（换行时隐藏行号）" aria-pressed="' + t.wrap + '">' + icon("wrap") + "</button></div></div>" + body +
    '<div class="terminal-status">' + icon("file") + "<span>" + languageName(languageMap[(t.name.split(".").pop() || "").toLowerCase()] || "plaintext") + "</span><span>" + (t.loaded?.type === "text" ? t.loaded.lines + " 行 · UTF-8 · " : "") + (t.pinned ? "已固定标签" : "临时预览 · 双击标签固定") + "</span></div>";
}

function renderQuickResults() {
  const p = currentProject();
  const query = ($("#quick-search").value || "").toLowerCase().trim();
  const files = [];
  for (const entries of p.dirCache.values()) for (const e of entries) if (!e.is_dir) files.push(e);
  const matches = files.filter((f) => !query || f.path.toLowerCase().includes(query)).slice(0, 70);
  $("#quick-results").innerHTML =
    matches.map((f) => '<button class="quick-result" data-file="' + esc(f.path) + '">' + fileSymbol(f.name) + "<span>" + esc(f.name) + "</span><small>" + esc(f.path.slice(p.rootPath.length + 1) || p.rootPath) + "</small></button>").join("") || '<div class="tree-empty">没有匹配的文件。先在左侧展开目录，再快速查找。</div>';
}
function openQuick() {
  if (activeWorkspace !== "project") return notify("选择一个项目后，即可快速打开文件。");
  $("#quick-title").textContent = (currentProject()?.name || "") + " · 快速打开文件";
  $("#quick-search").value = "";
  renderQuickResults();
  $("#quick-dialog").showModal();
  $("#quick-search").focus();
}
function openDirectory() {
  const dialog = $("#project-dialog");
  const nameInput = $("#project-name-input");
  const pathInput = $("#project-path-input");
  const errorEl = $("#project-form-error");
  const submitBtn = $("#project-submit");
  dialog.dataset.mode = "add";
  $("#project-dialog-title").textContent = "添加项目";
  $("#project-dialog-desc").textContent = "为本地目录指定一个名称，方便识别。";
  submitBtn.textContent = "添加";
  nameInput.value = "";
  pathInput.value = "";
  errorEl.hidden = true;
  dialog.showModal();
  nameInput.focus();
}
function openRenameDialog(id) {
  const p = projects.find((proj) => proj.id === id);
  if (!p) return;
  const dialog = $("#project-dialog");
  const nameInput = $("#project-name-input");
  const pathInput = $("#project-path-input");
  const errorEl = $("#project-form-error");
  const submitBtn = $("#project-submit");
  dialog.dataset.mode = "rename";
  dialog.dataset.projectId = id;
  $("#project-dialog-title").textContent = "重命名项目";
  $("#project-dialog-desc").textContent = "修改项目显示名称。";
  submitBtn.textContent = "保存";
  nameInput.value = p.name;
  pathInput.value = p.rootPath;
  errorEl.hidden = true;
  dialog.showModal();
  nameInput.focus();
  nameInput.select();
}
function toastErr(e) {
  notify(String(e?.message || e));
}

// ---------- 远程主机 ----------
function renderHosts() {
  syncGroupFilter(allVisibleHosts());
  $("#remote-host-count").textContent = hostPool().length;
  const term = ($("#host-search").value || "").toLowerCase().trim();
  const proto = $("#protocol-filter").value;
  const group = $("#group-filter").value;
  const all = allVisibleHosts();
  const matched = all.filter(
    (h) => (proto === "all" || h.protocol === proto) && (group === "all" || h.group === group) && (!term || [h.name, h.address, h.group].some((s) => (s || "").toLowerCase().includes(term)))
  );
  $("#visible-count").textContent = matched.length + "/" + all.length;
  const groups = [...new Set(matched.map((h) => h.group))];
  const current = activeTab();
  $("#host-list").innerHTML =
    groups
      .map((name) => {
        const items = matched.filter((h) => h.group === name);
        return '<div class="group-label">' + icon("chevron-down") + "<span>" + esc(name) + "</span><span>" + items.length + "</span></div>" + items.map((h) =>
          '<div class="host-entry"><button class="host-row'  + (current?.type === "terminal" && current.hostId === h.id && current.session ? " active" : "") + '" data-host="' + esc(h.id) + '" title="连接 ' + esc(h.name) + '"><span class="host-glyph">' + icon(h.protocol === "ssh" ? "server" : "monitor") +
          '</span><span class="host-info"><span class="host-name"><strong>' + esc(h.name) + '</strong><span class="protocol-tag">' + (h.protocol === "ssh" ? "SSH" : "瞬移") +
          '</span></span><span class="host-meta">' + esc((h.user && h.user !== "—" ? h.user + "@" : "") + h.address) + '</span></span><span class="connection-dot' + (hostIsOpen(h.id) ? " connected" : "") + '" title="' + (hostIsOpen(h.id) ? "会话已打开" : "尚未连接") + '"></span></button>' + (h.protocol === "unirc" ? '<button class="host-desktop-button" data-desktop-host="' + esc(h.id) + '" title="查看远程桌面" aria-label="查看 ' + esc(h.name) + ' 的桌面">' + icon("monitor") + '</button>' : '') + '</div>'
        ).join("");
      })
      .join("") || '<div class="empty-hosts">' + (hostPool().length ? '当前条件下没有主机。<button data-action="clear-filters">清除筛选</button><button data-action="manage-hosts">选择显示的主机</button>' : '添加一台主机，开始远程连接。<button data-action="new-host">添加远程主机</button>') + "</div>";
  renderPanelActions();
}
let lastGroups = "";
function syncGroupFilter(all) {
  const groups = [...new Set(all.map((h) => h.group))].sort();
  const key = groups.join("|");
  if (key === lastGroups) return;
  lastGroups = key;
  const sel = $("#group-filter");
  const cur = sel.value;
  sel.innerHTML = '<option value="all">全部分组</option>' + groups.map((g) => "<option>" + esc(g) + "</option>").join("");
  sel.value = groups.includes(cur) ? cur : "all";
}
function showHostPicker() {
  const vis = prefs.visibleHostIds;
  selectedHosts = new Set(vis || hostPool().map((h) => h.id));
  $("#picker-title").textContent = "选择显示的主机";
  renderHostPicker();
  $("#host-picker").showModal();
}
function renderHostPicker() {
  const pool = hostPool();
  $("#picker-count").textContent = "已选 " + selectedHosts.size + " / " + pool.length;
  $("#picker-list").innerHTML = [...new Set(pool.map((h) => h.group))].map((group) =>
    '<div class="picker-group">' + esc(group) + "</div>" + pool.filter((h) => h.group === group).map((h) =>
      '<label class="picker-row"><input type="checkbox" value="' + esc(h.id) + '"' + (selectedHosts.has(h.id) ? " checked" : "") + '><span class="host-info">' + esc(h.name) + '<span class="host-meta">' + esc(h.address) + (hostIsOpen(h.id) ? " · 会话已打开" : "") + '</span></span><span class="protocol-tag">' + (h.protocol === "ssh" ? "SSH" : "瞬移") + "</span></label>"
    ).join("")
  ).join("");
}
function clearFilters() {
  $("#host-search").value = "";
  $("#protocol-filter").value = "all";
  $("#group-filter").value = "all";
  renderHosts();
  saveWorkspace();
}
function renderPanelActions() {} // 预留

// ---------- 主机新建 / 编辑 ----------
let editingHostId = null;
let hostFormRevision = 0;
function showHostFormError(error) {
  const alert = $("#host-form-error");
  alert.textContent = error?.message || String(error);
  alert.hidden = false;
  alert.scrollIntoView({ block: "nearest" });
}
function prepareHostDialog() {
  hostFormRevision++;
  const f = $("#host-form");
  f.querySelectorAll("input").forEach((input) => input.setCustomValidity(""));
  $("#host-form-error").hidden = true;
  $("#host-save-only").hidden = !!editingHostId;
  $("#host-submit").value = editingHostId ? "save" : "connect";
  $("#host-dialog [data-close]").setAttribute("aria-label", editingHostId ? "关闭编辑主机" : "关闭新建主机");
  $("#host-group-suggestions").innerHTML = [...new Set(hosts.map((h) => h.group || "默认"))]
    .sort((a, b) => a.localeCompare(b, "zh-CN"))
    .map((group) => `<option value="${esc(group)}"></option>`).join("");
  syncHostDialogProto();
  $("#host-dialog").showModal();
  $(".host-form-body").scrollTop = 0;
  f.elements.namedItem("name").focus();
}
function openHostDialog() {
  editingHostId = null;
  $("#host-dialog-title").textContent = "新建主机";
  $("#host-submit").textContent = "保存并连接";
  const f = $("#host-form");
  f.reset();
  f.elements.namedItem("port").value = 22;
  prepareHostDialog();
}
function openHostEdit(id) {
  const h = hosts.find((x) => x.id === id);
  if (!h) return notify("在线设备由 Agent 管理，暂不支持编辑");
  editingHostId = id;
  $("#host-dialog-title").textContent = "编辑主机";
  $("#host-submit").textContent = "保存更改";
  const f = $("#host-form");
  f.reset();
  f.elements.namedItem("name").value = h.name || "";
  f.elements.namedItem("group").value = h.group || "";
  f.elements.namedItem("proto").value = h.proto || "ssh";
  f.elements.namedItem("host").value = h.host || "";
  f.elements.namedItem("port").value = h.port || 22;
  f.elements.namedItem("username").value = h.username || "";
  f.elements.namedItem("authType").value = h.authType || "password";
  f.elements.namedItem("password").value = h.password || "";
  f.elements.namedItem("keyPath").value = h.keyPath || "";
  f.elements.namedItem("deviceId").value = h.deviceId || "";
  f.elements.namedItem("deviceAuth").value = h.deviceAuth || "certificate";
  f.elements.namedItem("certificatePath").value = h.certificatePath || "";
  prepareHostDialog();
}
function maskHostPassword() {
  $("#host-password").type = "password";
  $("#host-password-toggle").innerHTML = icon("eye");
  $("#host-password-toggle").setAttribute("aria-label", "显示密码");
  $("#host-password-toggle").setAttribute("aria-pressed", "false");
}
function syncHostDialogProto() {
  const f = $("#host-form");
  const proto = f.elements.namedItem("proto").value;
  f.querySelectorAll("[data-host-proto]").forEach((fieldset) => {
    fieldset.hidden = fieldset.dataset.hostProto !== proto;
    fieldset.disabled = fieldset.hidden;
  });
  f.elements.namedItem("host").required = proto === "ssh";
  f.elements.namedItem("deviceId").required = proto === "unirc";
  $("#host-protocol-hint").textContent = proto === "ssh"
    ? "通过 IP 地址或域名，直接连接远程主机。"
    : "通过已接入瞬移服务器的 Agent 连接远程设备。";
  $("#host-unirc-note").hidden = proto !== "unirc";
  const auth = f.elements.namedItem("authType").value;
  $(".auth-password").hidden = auth !== "password";
  $(".auth-key").hidden = auth !== "key";
  f.elements.namedItem("password").disabled = auth !== "password";
  f.elements.namedItem("keyPath").disabled = auth !== "key";
  f.elements.namedItem("keyPath").required = proto === "ssh" && auth === "key";
  const deviceAuth = f.elements.namedItem("deviceAuth").value;
  $("#certificate-field").hidden = deviceAuth !== "certificate";
  $("#temporary-field").hidden = deviceAuth !== "temporary";
  f.elements.namedItem("certificatePath").disabled = proto !== "unirc" || deviceAuth !== "certificate";
  f.elements.namedItem("temporaryPassword").disabled = proto !== "unirc" || deviceAuth !== "temporary";
  maskHostPassword();
}
$("#host-form").addEventListener("change", (e) => {
  hostFormRevision++;
  if (["proto", "authType", "deviceAuth"].includes(e.target.name)) syncHostDialogProto();
});
$("#host-form").addEventListener("input", (e) => {
  hostFormRevision++;
  if (e.target instanceof HTMLInputElement) e.target.setCustomValidity("");
  $("#host-form-error").hidden = true;
});
$("#host-password-toggle").onclick = () => {
  const reveal = $("#host-password").type === "password";
  $("#host-password").type = reveal ? "text" : "password";
  $("#host-password-toggle").innerHTML = icon(reveal ? "eye-off" : "eye");
  $("#host-password-toggle").setAttribute("aria-label", reveal ? "隐藏密码" : "显示密码");
  $("#host-password-toggle").setAttribute("aria-pressed", String(reveal));
};
$("#host-dialog").addEventListener("close", () => {
  hostFormRevision++;
  maskHostPassword();
  $("#host-form").reset();
  editingHostId = null;
});
$("#host-form").onsubmit = async (e) => {
  e.preventDefault();
  const f = e.target;
  const revision = ++hostFormRevision;
  const submittedHostId = editingHostId;
  // Read the controls directly to preserve values while their protocol is inactive.
  const get = (k) => String(f.elements.namedItem(k).value ?? "").trim();
  const requiredFields = [["name", "请输入主机名称。"], get("proto") === "ssh"
    ? ["host", "请输入主机 IP 地址或域名。"] : ["deviceId", "请输入设备 ID。"]];
  if (get("proto") === "ssh" && get("authType") === "key") requiredFields.push(["keyPath", "请输入私钥文件路径。"]);
  for (const [field, message] of requiredFields) {
    if (!get(field)) {
      f.elements.namedItem(field).setCustomValidity(message);
      f.elements.namedItem(field).reportValidity();
      return;
    }
  }
  const justAdded = !editingHostId;
  const shouldConnect = justAdded && e.submitter?.value === "connect";
  if (get("proto") === "unirc") {
    try {
      if (get("deviceAuth") === "certificate") {
        if (!get("certificatePath")) throw new Error("请选择设备所有者提供的连接证书。");
        if (inTauri) {
          const info = await invoke("inspect_access_certificate", { path: get("certificatePath") });
          if (revision !== hostFormRevision || !$("#host-dialog").open) return;
          if (info.device_id !== get("deviceId")) throw new Error("连接证书与设备 ID 不匹配，请重新选择证书。");
        }
      }
      if (!/^rc-[a-f0-9]{64}$/.test(get("deviceId"))) throw new Error("请填写完整的新版本设备 ID（rc- 后有 64 位字符）。");
    } catch (error) {
      if (revision === hostFormRevision && $("#host-dialog").open) showHostFormError(error);
      return;
    }
  }
  const host = {
    ...hosts.find((h) => h.id === submittedHostId),
    id: submittedHostId || "h-" + Date.now().toString(36) + "-" + sequence++,
    name: get("name"),
    group: get("group") || "默认",
    proto: get("proto"),
    host: get("host"),
    port: Number(get("port")) || 22,
    username: get("username"),
    authType: get("authType") || "password",
    password: f.elements.namedItem("password").value,
    keyPath: get("keyPath"),
    deviceId: get("deviceId"),
    deviceAuth: get("deviceAuth"),
    certificatePath: get("certificatePath"),
  };
  delete host.temporaryPassword;
  delete host.privateKey;
  const temporaryPassword = f.elements.namedItem("temporaryPassword").value;
  const nextHosts = justAdded ? [...hosts, host] : hosts.map((h) => h.id === submittedHostId ? host : h);
  if (!saveJson(hostsKey, nextHosts)) {
    showHostFormError("主机未能保存，填写内容已保留。请重试。");
    return;
  }
  hosts = nextHosts;
  if (shouldConnect && host.proto === "unirc" && host.deviceAuth === "temporary" && temporaryPassword) temporaryPasswords.set(host.id, temporaryPassword);
  if (justAdded && Array.isArray(prefs.visibleHostIds)) prefs.visibleHostIds.push(host.id);
  $("#host-dialog").close();
  renderHosts();
  editingHostId = null;
  if (shouldConnect) connectHost(host.id);
  else notify(justAdded ? "主机已保存，可从远程主机列表连接。" : "主机信息已更新。");
};

// Certificate selection reads metadata through native code; private keys never enter the webview.
$("#choose-certificate").onclick = async () => {
  if (!inTauri) return notify("请在 Mac 客户端中选择连接证书。");
  const button = $("#choose-certificate"); button.disabled = true;
  const revision = ++hostFormRevision;
  try {
    const info = await invoke("pick_access_certificate");
    if (revision !== hostFormRevision || !$("#host-dialog").open) return;
    if (info) {
      const f = $("#host-form");
      f.elements.namedItem("certificatePath").value = info.path;
      f.elements.namedItem("deviceId").value = info.device_id;
      f.elements.namedItem("deviceId").setCustomValidity("");
      f.elements.namedItem("certificatePath").setCustomValidity("");
      $("#host-form-error").hidden = true;
    }
  } catch (error) { if (revision === hostFormRevision && $("#host-dialog").open) showHostFormError(error); }
  finally { button.disabled = false; }
};
let temporaryRequest = null;
function requestTemporaryPassword(hostId, connect) {
  temporaryRequest = { hostId, connect };
  $("#temporary-form").reset();
  $("#temporary-target").textContent = "连接「" + (hosts.find((h) => h.id === hostId)?.name || "远程设备") + "」";
  $("#temporary-dialog").showModal();
  $("#temporary-form input").focus();
}
$("#temporary-form").onsubmit = (e) => {
  e.preventDefault();
  const request = temporaryRequest;
  if (!request) return;
  temporaryPasswords.set(request.hostId, e.target.elements.namedItem("password").value);
  $("#temporary-dialog").close();
  request.connect();
};
$("#temporary-dialog").addEventListener("close", () => { $("#temporary-form").reset(); temporaryRequest = null; });
function exportableHost(host) {
  // Whitelist the configuration schema. A pasted/imported private key cannot escape into an export.
  const result = {};
  for (const field of ["name", "group", "proto", "host", "port", "username", "authType", "keyPath", "deviceId", "deviceAuth", "certificatePath"]) {
    if (host[field] !== undefined) result[field] = host[field];
  }
  return result;
}

// ---------- 导入 / 导出 ----------
$("#btn-export-hosts").onclick = async () => {
  if (!hosts.length) return notify("还没有可导出的服务器");
  try {
    const where = await invoke("export_hosts_text", { content: JSON.stringify(hosts.map(exportableHost), null, 2) });
    if (where) notify("已导出到 " + where);
  } catch (e) {
    toastErr(e);
  }
};
$("#btn-import-hosts").onclick = async () => {
  try {
    const text = await invoke("import_hosts_text");
    if (!text) return;
    const arr = JSON.parse(text);
    if (!Array.isArray(arr)) throw new Error("格式不对：需要 JSON 数组");
    let added = 0;
    for (const item of arr) {
      if (!item || !item.name || (!item.host && !item.deviceId)) continue;
      if (hosts.some((x) => x.name === item.name && x.host === item.host && x.deviceId === item.deviceId)) continue;
      hosts.push({ ...exportableHost(item), id: "h-" + Date.now().toString(36) + "-" + sequence++, snippets: [] });
      added++;
    }
    saveJson(hostsKey, hosts);
    renderHosts();
    notify(`导入完成：新增 ${added} 台`);
  } catch (e) {
    toastErr(e);
  }
};

// ---------- 快捷指令面板（右侧） ----------
const commandCompact = matchMedia("(max-width: 1179px)");
commandPanelOpen = !commandCompact.matches;
function saveCommands() {
  return saveJson(commandsKey, commandRecords);
}
function syncCommandPanel() {
  const t = activeTab();
  const isTerminal = t?.type === "terminal";
  $("#command-panel").hidden = !isTerminal || !commandPanelOpen;
  $("#main-body").classList.toggle("commands-open", Boolean(isTerminal && commandPanelOpen));
  const toggle = $('[data-action="toggle-commands"]');
  if (toggle) {
    toggle.setAttribute("aria-expanded", String(commandPanelOpen));
    toggle.classList.toggle("active", commandPanelOpen);
  }
  if (isTerminal) {
    $("#command-target-name").textContent = t.title;
    $("#command-target-project").textContent = (tabWorkspace(t)?.name || "当前工作区") + " · " + sessionAddress(t);
  }
}
function toggleCommandPanel(force) {
  commandPanelOpen = typeof force === "boolean" ? force : !commandPanelOpen;
  syncCommandPanel();
  if (commandPanelOpen) ($("#command-editor").hidden ? $("#command-search") : $("#command-name")).focus();
}
function renderCommandList() {
  const query = ($("#command-search").value || "").trim().toLowerCase();
  const matches = commandRecords.filter((item) => !query || [item.name, item.command].some((v) => v.toLowerCase().includes(query)));
  $("#command-count").textContent = query ? "找到 " + matches.length + " / " + commandRecords.length + " 条" : "全部指令 · " + commandRecords.length;
  $("#command-list").innerHTML =
    matches.map((item) =>
      '<div class="command-row" role="listitem"><button class="command-send" data-send-command="' + esc(item.id) + '" title="' + esc(item.command) + '\n点击发送到当前终端并回车">' +
      '<span class="command-name">' + esc(item.name) + "</span><code>" + esc(item.command) + '</code></button><div class="command-row-tools">' +
      '<button class="icon-button" data-edit-command="' + esc(item.id) + '" title="编辑指令" aria-label="编辑指令">' + icon("edit") + '</button>' +
      '<button class="icon-button command-delete" data-delete-command="' + esc(item.id) + '" title="删除指令" aria-label="删除指令">' + icon("trash") + "</button></div></div>"
    ).join("") ||
    (query
      ? '<div class="command-empty"><strong>没有匹配的指令</strong><p>试试其他名称或命令关键词。</p><button class="text-button" id="clear-command-search">清除搜索</button></div>'
      : '<div class="command-empty">' + icon("terminal") + '<strong>把常用命令放在这里</strong><p>添加名称和命令，之后点一下即可发送到当前终端。</p><button class="primary-button" id="add-first-command">添加第一条指令</button></div>');
}
function openCommandEditor(id = null) {
  const item = id ? commandRecords.find((r) => r.id === id) : null;
  if (id && !item) return;
  editingCommandId = id;
  $("#command-name").value = item?.name || "";
  $("#command-text").value = item?.command || "";
  $("#command-editor-title").textContent = item ? "编辑指令" : "添加指令";
  $("#command-library").hidden = true;
  $("#command-editor").hidden = false;
  $("#add-command").hidden = true;
  $("#command-name").focus();
}
function closeCommandEditor() {
  editingCommandId = null;
  $("#command-editor").hidden = true;
  $("#command-library").hidden = false;
  $("#add-command").hidden = false;
  renderCommandList();
  $("#command-search").focus();
}
function removeCommand(id) {
  const index = commandRecords.findIndex((item) => item.id === id);
  if (index < 0) return;
  deletedCommand = { item: commandRecords[index], index };
  commandRecords.splice(index, 1);
  saveCommands();
  renderCommandList();
  $("#command-undo-message").textContent = "已删除「" + deletedCommand.item.name + "」";
  $("#command-undo").hidden = false;
  $("#undo-command").focus();
}
async function sendSavedCommand(id) {
  const item = commandRecords.find((r) => r.id === id);
  const target = activeTab();
  if (!item || target?.type !== "terminal" || !target.session) return notify("先打开可用的终端，再发送指令。");
  try {
    await sendInput(target, item.command.replace(/\r?\n/g, "\r") + "\r");
    notify("已发送「" + item.name + "」至 " + (tabWorkspace(target)?.name || "") + " · " + target.title);
    if (activeTab() === target) target.term.focus();
  } catch (e) { toastErr(e); }
}
$("#add-command").onclick = () => openCommandEditor();
$("#close-command-panel").onclick = () => toggleCommandPanel(false);
$("#command-search").oninput = renderCommandList;
$("#cancel-command").onclick = closeCommandEditor;
$("#cancel-command-top").onclick = closeCommandEditor;
$("#command-editor").onsubmit = (event) => {
  event.preventDefault();
  const name = $("#command-name").value.trim();
  const command = $("#command-text").value.replace(/\r\n?/g, "\n").trim();
  if (!name || !command) return;
  const item = { id: editingCommandId || "cmd-" + Date.now().toString(36) + "-" + sequence++, name, command };
  if (editingCommandId) {
    const i = commandRecords.findIndex((r) => r.id === editingCommandId);
    if (i < 0) return;
    commandRecords[i] = item;
  } else commandRecords.unshift(item);
  saveCommands();
  closeCommandEditor();
  notify("已保存「" + name + "」，点击指令即可发送。");
};
$("#command-list").onclick = (event) => {
  const edit = event.target.closest("[data-edit-command]");
  const remove = event.target.closest("[data-delete-command]");
  const send = event.target.closest("[data-send-command]");
  if (edit) openCommandEditor(edit.dataset.editCommand);
  else if (remove) removeCommand(remove.dataset.deleteCommand);
  else if (send) sendSavedCommand(send.dataset.sendCommand);
  else if (event.target.closest("#clear-command-search")) {
    $("#command-search").value = "";
    renderCommandList();
    $("#command-search").focus();
  } else if (event.target.closest("#add-first-command")) openCommandEditor();
};
$("#undo-command").onclick = () => {
  if (!deletedCommand) return;
  commandRecords.splice(Math.min(deletedCommand.index, commandRecords.length), 0, deletedCommand.item);
  deletedCommand = null;
  saveCommands();
  $("#command-undo").hidden = true;
  renderCommandList();
  $("#command-search").focus();
};
commandCompact.addEventListener("change", (event) => {
  commandPanelOpen = !event.matches;
  syncCommandPanel();
});

// ---------- 设置 ----------
let settingsHint = null;
function openSettings(hint) {
  settingsHint = hint || null;
  const f = $("#settings-form");
  f.elements.namedItem("serverUrl").value = settings.serverUrl;
  f.elements.namedItem("token").value = settings.token;
  $("#settings-dialog").showModal();
}
$("#settings-form").onsubmit = (e) => {
  e.preventDefault();
  const f = e.target;
  settings.serverUrl = f.elements.namedItem("serverUrl").value.trim().replace(/\/$/, "");
  settings.token = f.elements.namedItem("token").value.trim();
  saveJson(settingsKey, settings);
  $("#settings-dialog").close();
  notify("已保存设置");
  if (settingsHint) notify(settingsHint);
  settingsHint = null;
};

// ---------- 本机免账号接入 ----------
let localDeviceRunning = false;
let devicePoll = null;
const temporaryStates = { none: "未生成", unused: "等待使用", used: "已使用 · 不能重连", expired: "已过期" };
async function refreshDeviceStatus() {
  const status = await invoke("device_status");
  const c = status.credentials;
  localDeviceRunning = status.running;
  $("#desktop-permission").disabled = status.running || !status.desktop_available;
  if (status.running) $("#desktop-permission").value = status.desktop_permission;
  $("#desktop-availability").textContent = status.desktop_available ? "启用桌面后，有效证书或临时密码可获得所选权限。更改权限前请停止共享。" : "远程桌面需要安装瞬移远控预览版。";
  $(".desktop-permissions").hidden = status.desktop_platform !== "macos";
  if (status.desktop_available && status.desktop_platform === "linux") $("#desktop-availability").textContent += " Linux 预览版需要已登录的 X11 桌面。";
  if (status.desktop_available && status.desktop_platform === "windows") $("#desktop-availability").textContent += " Windows 控制限当前登录会话，系统提权界面不在共享范围内。";
  $("#local-device-id").value = c.device_id;
  $("#local-temporary-state").textContent = temporaryStates[c.temporary_state] || "未知";
  $("#toggle-device-sharing").textContent = status.running ? "停止共享" : "开始共享";
  $("#device-runtime").textContent = status.running ? `共享服务运行中 · 终端用户 ${status.user}` : `共享已关闭 · 启用后使用 ${status.user} 的终端`;
  $("#certificate-rotation").value = c.rotation_hours ? String(c.rotation_hours) : "off";
  if (status.error) throw new Error(status.error);
}
async function deviceAction(action) {
  $("#device-error").hidden = true;
  try { await action(); await refreshDeviceStatus(); }
  catch (error) { $("#device-error").textContent = error?.message || String(error); $("#device-error").hidden = false; }
}
$("#open-device-access").onclick = async () => {
  if (!inTauri) return notify("本机接入与凭据管理需要在 Mac 客户端中使用。");
  $("#settings-dialog").close();
  $("#device-dialog").showModal();
  await deviceAction(async () => {});
  devicePoll = setInterval(() => deviceAction(async () => {}), 3000);
};
$("#device-dialog").addEventListener("close", () => {
  clearInterval(devicePoll); devicePoll = null;
  $("#local-temporary-password").value = "";
  $("#device-password-result").hidden = true;
});
document.querySelectorAll("[data-desktop-permission]").forEach((button) => { button.onclick = () => deviceAction(() => invoke("desktop_permissions", { kind: button.dataset.desktopPermission })); });
$("#toggle-device-sharing").onclick = async (e) => {
  const button = e.currentTarget; button.disabled = true;
  await deviceAction(async () => {
    if (localDeviceRunning) await invoke("device_stop");
    else {
      if (!settings.serverUrl) throw new Error("请先在设置中保存中继服务器地址，再开始共享。");
      await invoke("device_start", { server: settings.serverUrl, token: settings.token, desktopPermission: $("#desktop-permission").value });
    }
  });
  button.disabled = false;
};
$("#copy-device-id").onclick = () => deviceAction(async () => { await navigator.clipboard.writeText($("#local-device-id").value); notify("设备 ID 已复制"); });
$("#export-device-certificate").onclick = () => deviceAction(async () => { const path = await invoke("device_export_certificate"); if (path) notify("连接证书已导出，请仅交给受信任的人。"); });
$("#rotate-device-certificate").onclick = () => deviceAction(async () => { await invoke("device_rotate_certificate"); notify("证书已轮换，请重新导出。已有终端继续运行。"); });
$("#certificate-rotation").onchange = (e) => { const value = e.target.value; deviceAction(() => invoke("device_set_rotation", { hours: value === "off" ? null : Number(value) })); };
$("#generate-device-password").onclick = async (e) => {
  const button = e.currentTarget; button.disabled = true;
  await deviceAction(async () => {
    const password = await invoke("device_temporary_password", { minutes: Number($("#temporary-duration").value) });
    $("#local-temporary-password").value = password;
    $("#device-password-result").hidden = false;
  }); button.disabled = false;
};
$("#copy-device-password").onclick = () => deviceAction(async () => { await navigator.clipboard.writeText($("#local-temporary-password").value); notify("临时密码已复制"); });
$("#revoke-device-password").onclick = () => deviceAction(async () => { await invoke("device_revoke_temporary"); $("#local-temporary-password").value = ""; $("#device-password-result").hidden = true; notify("未使用的临时密码已作废"); });

// ---------- 交互绑定 ----------
$("#open-terminal").onclick = openWorkspaceTerminal;
$("#help-button").onclick = () => $("#help-dialog").showModal();
$("#remote-workspace-button").onclick = () => selectRemoteWorkspace({ toggle: true });
$("#collapse-projects").onclick = () => {
  projectsSectionExpanded = !projectsSectionExpanded;
  renderProjects(); saveWorkspace();
};
$("#new-host").onclick = () => openHostDialog();
$("#project-list").onclick = (e) => {
  const terminal = e.target.closest("[data-project-terminal]");
  if (terminal) {
    selectProject(terminal.dataset.projectTerminal, false, { restore: false });
    newTerminal();
    return;
  }
  const row = e.target.closest("[data-project]");
  if (row) {
    selectProject(row.dataset.project, true);
    $("#project-list").querySelector('[data-project="' + row.dataset.project + '"]')?.focus();
  }
};
$("#open-folder").onclick = openDirectory;
$("#project-pick-folder").onclick = () => {
  invoke("pick_folder").then((dir) => {
    if (dir) {
      $("#project-path-input").value = dir;
      if (!$("#project-name-input").value) {
        $("#project-name-input").value = dir.split("/").filter(Boolean).pop() || "";
      }
    }
  }).catch((e) => toastErr(e));
};
$("#project-form").onsubmit = (e) => {
  e.preventDefault();
  const dialog = $("#project-dialog");
  const name = $("#project-name-input").value.trim();
  const path = $("#project-path-input").value.trim();
  const errorEl = $("#project-form-error");
  if (!name) { errorEl.textContent = "请输入项目名称"; errorEl.hidden = false; return; }
  if (dialog.dataset.mode === "rename") {
    renameProject(dialog.dataset.projectId, name);
  } else {
    if (!path) { errorEl.textContent = "请选择目录"; errorEl.hidden = false; return; }
    addProject(path, name);
  }
  dialog.close();
};
$("#quick-open").onclick = openQuick;
$("#root-toggle").onclick = () => {
  currentProject().rootExpanded = !currentProject().rootExpanded;
  renderTree(); saveWorkspace();
};
$("#refresh-files").onclick = async () => {
  const p = currentProject();
  await Promise.allSettled([...p.dirLoading.values()]);
  p.dirCache.clear(); p.dirErrors.clear();
  if (currentProject() === p) renderTree();
};
$("#collapse-tree").onclick = () => {
  currentProject().expandedDirs.clear();
  renderTree();
  saveWorkspace();
};
$("#file-filter").oninput = () => {
  renderTree();
  saveWorkspace();
};
$("#host-search").oninput = () => {
  renderHosts();
  saveWorkspace();
};
$("#protocol-filter").onchange = () => {
  renderHosts();
  saveWorkspace();
};
$("#group-filter").onchange = () => {
  renderHosts();
  saveWorkspace();
};
$("#manage-hosts").onclick = showHostPicker;
$("#quick-search").oninput = renderQuickResults;
$("#settings-button").onclick = () => openSettings();
$("#toggle-sidebar").onclick = () => $("#workspace").classList.toggle("sidebar-hidden");
$("#select-all").onclick = () => {
  selectedHosts = new Set(hostPool().map((h) => h.id));
  renderHostPicker();
};
$("#select-none").onclick = () => {
  selectedHosts.clear();
  renderHostPicker();
};
$("#picker-list").onchange = (e) => {
  if (e.target.type === "checkbox") {
    e.target.checked ? selectedHosts.add(e.target.value) : selectedHosts.delete(e.target.value);
    $("#picker-count").textContent = "已选 " + selectedHosts.size + " / " + hostPool().length;
  }
};
$("#apply-host-selection").onclick = () => {
  prefs.visibleHostIds = selectedHosts.size >= hostPool().length ? null : [...selectedHosts];
  saveWorkspace();
  renderHosts();
  $("#host-picker").close();
  notify("已更新显示的主机，会话保持不变。");
};
$("#tabbar").addEventListener("click", (e) => {
  const close = e.target.closest("[data-close-tab]");
  if (close) {
    closeTab(close.dataset.closeTab);
    return;
  }
  const item = e.target.closest("[data-tab]");
  if (!item) return;
  const id = item.dataset.tab;
  const t = tabs.find((tab) => tab.id === id);
  const now = Date.now();
  if (t?.type === "file" && lastTabClick.id === id && now - lastTabClick.time < 450) t.pinned = true;
  lastTabClick = { id, time: now };
  if (activeId !== id) activateTab(id);
});
let lastTabClick = { id: null, time: 0 };
$("#tabbar").addEventListener("keydown", (e) => {
  const row = e.target.closest('[role="tab"]');
  if (!row || e.target.closest("button")) return;
  const owned = workspaceTabs(), index = owned.findIndex((t) => t.id === row.dataset.tab);
  let next;
  if (e.key === "ArrowRight") next = owned[(index + 1) % owned.length];
  else if (e.key === "ArrowLeft") next = owned[(index - 1 + owned.length) % owned.length];
  else if (e.key === "Home") next = owned[0];
  else if (e.key === "End") next = owned[owned.length - 1];
  else if (["Enter", " "].includes(e.key)) next = owned[index];
  if (next) {
    e.preventDefault(); activateTab(next.id, { focusTerminal: false });
    [...$("#tabbar").querySelectorAll('[role="tab"]')].find((el) => el.dataset.tab === next.id)?.focus();
  }
});
$("#tabbar").addEventListener("dblclick", (e) => {
  const id = e.target.closest("[data-tab]")?.dataset.tab;
  const t = tabs.find((tab) => tab.id === id);
  if (t?.type === "file") {
    t.pinned = true;
    renderTabs();
    renderFileTab(t);
    notify("文件标签已固定。");
  }
});
$("#file-tree").addEventListener("click", (e) => {
  const row = e.target.closest("[data-path]");
  if (!row) return;
  row.dataset.directory === "true" ? toggleDirectory(row.dataset.path) : openFile(row.dataset.path);
});
$("#file-tree").addEventListener("dblclick", (e) => {
  const row = e.target.closest('[data-directory="false"]');
  if (row) openFile(row.dataset.path, true);
});
$("#file-tree").addEventListener("keydown", (e) => {
  const row = e.target.closest("[data-path]");
  if (!row) return;
  const rows = [...$("#file-tree").querySelectorAll("[data-path]")];
  const index = rows.indexOf(row), directory = row.dataset.directory === "true";
  if (["Enter", " ", "ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight"].includes(e.key)) e.preventDefault();
  if (e.key === "ArrowDown") rows[Math.min(index + 1, rows.length - 1)]?.focus();
  else if (e.key === "ArrowUp") rows[Math.max(index - 1, 0)]?.focus();
  else if (["Enter", " "].includes(e.key)) directory ? toggleDirectory(row.dataset.path) : openFile(row.dataset.path);
  else if (directory && ((e.key === "ArrowRight" && row.getAttribute("aria-expanded") === "false") || (e.key === "ArrowLeft" && row.getAttribute("aria-expanded") === "true"))) toggleDirectory(row.dataset.path);
});
let contextMenu;
function closeContextMenu() {
  contextMenu?.remove();
  contextMenu = null;
}
$("#file-tree").addEventListener("contextmenu", (e) => {
  const row = e.target.closest("[data-path]");
  if (!row) return;
  e.preventDefault();
  closeContextMenu();
  const directory = row.dataset.directory === "true" ? row.dataset.path : row.dataset.path.split("/").slice(0, -1).join("/");
  contextMenu = document.createElement("div");
  contextMenu.className = "context-menu";
  contextMenu.innerHTML = "<button>" + icon("terminal") + "在此目录打开终端</button>";
  contextMenu.style.left = Math.min(e.clientX, innerWidth - 205) + "px";
  contextMenu.style.top = Math.min(e.clientY, innerHeight - 70) + "px";
  contextMenu.querySelector("button").onclick = () => {
    newTerminal(directory);
    closeContextMenu();
  };
  document.body.appendChild(contextMenu);
  contextMenu.querySelector("button").focus();
});
// 主机右键：编辑 / 删除（保存的主机）
let hostMenu;
$("#host-list").addEventListener("contextmenu", (e) => {
  const row = e.target.closest("[data-host]");
  if (!row) return;
  e.preventDefault();
  hostMenu?.remove();
  hostMenu = document.createElement("div");
  hostMenu.className = "context-menu";
  hostMenu.innerHTML = (poolEntry(row.dataset.host)?.protocol === "unirc" ? "<button data-hm='view-desktop'>" + icon("monitor") + "查看远程桌面</button><button data-hm='control-desktop'>" + icon("monitor") + "控制远程桌面</button>" : "") + "<button data-hm='edit'>" + icon("edit") + "编辑主机</button><button data-hm='del'>" + icon("trash") + "删除主机</button>";
  hostMenu.style.left = Math.min(e.clientX, innerWidth - 205) + "px";
  hostMenu.style.top = Math.min(e.clientY, innerHeight - 100) + "px";
  hostMenu.onclick = (ev) => {
    const act = ev.target.closest("[data-hm]")?.dataset.hm;
    hostMenu.remove();
    hostMenu = null;
    const savedId = row.dataset.host;
    if (act === "view-desktop" || act === "control-desktop") { openDesktopHost(savedId, act === "control-desktop"); return; }
    if (act === "edit") openHostEdit(savedId);
    else if (act === "del") {
      if (!savedId.startsWith("h-")) return notify("在线设备由 Agent 管理，暂不支持删除");
      if (!confirm("删除主机？")) return;
      hosts = hosts.filter((x) => x.id !== savedId);
      saveJson(hostsKey, hosts);
      renderHosts();
    }
  };
  document.body.appendChild(hostMenu);
});
// 项目右键：移除项目
let projectMenu;
$("#project-list").addEventListener("contextmenu", (e) => {
  const row = e.target.closest("[data-project]");
  if (!row) return;
  e.preventDefault();
  projectMenu?.remove();
  projectMenu = document.createElement("div");
  projectMenu.className = "context-menu";
  projectMenu.innerHTML = "<button data-pm='rename'>" + icon("edit") + "重命名</button><button data-pm='remove'>" + icon("trash") + "移除项目</button>";
  projectMenu.style.left = Math.min(e.clientX, innerWidth - 205) + "px";
  projectMenu.style.top = Math.min(e.clientY, innerHeight - 70) + "px";
  projectMenu.onclick = (ev) => {
    const act = ev.target.closest("[data-pm]")?.dataset.pm;
    projectMenu.remove();
    projectMenu = null;
    const projectId = row.dataset.project;
    if (act === "rename") openRenameDialog(projectId);
    else if (act === "remove") {
      const name = projects.find((p) => p.id === projectId)?.name || "";
      if (!confirm("移除项目「" + name + "」？已打开的终端将被关闭。")) return;
      removeProject(projectId);
    }
  };
  document.body.appendChild(projectMenu);
});
document.addEventListener("click", (e) => {
  if (!e.target.closest(".context-menu")) {
    closeContextMenu();
    hostMenu?.remove();
    hostMenu = null;
    projectMenu?.remove();
    projectMenu = null;
  }
  const close = e.target.closest("[data-close]");
  if (close) $("#" + close.dataset.close).close();
  const desktopHost = e.target.closest("[data-desktop-host]");
  if (desktopHost) openDesktopHost(desktopHost.dataset.desktopHost);
  const host = e.target.closest("[data-host]");
  if (host) connectHost(host.dataset.host);
  const file = e.target.closest("[data-file]");
  if (file) {
    $("#quick-dialog").close();
    openFile(file.dataset.file);
  }
  const action = e.target.closest("[data-action]")?.dataset.action;
  if (action === "toggle-commands") toggleCommandPanel();
  if (action === "new-terminal") newTerminal();
  if (action === "choose-remote") chooseRemoteHost();
  if (action === "new-host") openHostDialog();
  if (action === "retry-terminal") retryTerminal();
  if (action === "open-folder") openDirectory();
  if (action === "manage-hosts") showHostPicker();
  if (action === "clear-filters") clearFilters();
  if (action === "wrap" && activeTab()?.type === "file") {
    activeTab().wrap = !activeTab().wrap;
    renderMain();
  }
  if (action === "markdown" && activeTab()?.type === "file") {
    activeTab().markdown = !activeTab().markdown;
    renderMain();
  }
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeContextMenu();
  if (document.querySelector("dialog[open]")) return;
  if ((e.metaKey || e.ctrlKey) && !e.shiftKey && ["b", "o", "p"].includes(e.key.toLowerCase())) {
    e.preventDefault();
    if (e.key.toLowerCase() === "b") $("#workspace").classList.toggle("sidebar-hidden");
    else if (e.key.toLowerCase() === "o") openDirectory();
    else openQuick();
  }
});

// 侧栏拖宽
function setSidebarWidth(width) {
  const clamped = Math.max(230, Math.min(420, Number(width) || 280, innerWidth - 370));
  document.documentElement.style.setProperty("--sidebar-width", clamped + "px");
  $("#sidebar-resize").setAttribute("aria-valuenow", String(Math.round(clamped)));
}
const resizer = $("#sidebar-resize");
resizer.addEventListener("pointerdown", (e) => {
  resizer.setPointerCapture(e.pointerId);
  resizer._drag = true;
  document.body.style.cursor = "col-resize";
});
resizer.addEventListener("pointermove", (e) => {
  if (resizer._drag) setSidebarWidth(e.clientX - $("#workspace").getBoundingClientRect().left);
});
resizer.addEventListener("pointerup", () => {
  resizer._drag = false;
  document.body.style.cursor = "";
  saveWorkspace();
});
resizer.addEventListener("pointercancel", () => {
  resizer._drag = false;
  document.body.style.cursor = "";
});
resizer.addEventListener("keydown", (e) => {
  if (["ArrowLeft", "ArrowRight"].includes(e.key)) {
    e.preventDefault();
    setSidebarWidth($("#sidebar").getBoundingClientRect().width + (e.key === "ArrowLeft" ? -10 : 10));
    saveWorkspace();
  }
});

// ---------- Native terminal events must be ready before any connection opens. ----------
const terminalListenersReady = inTauri ? Promise.all([
  listen("term:data", (ev) => routeTerminalEvent({ ...ev.payload, type: "data" })),
  listen("term:closed", (ev) => routeTerminalEvent({ ...ev.payload, type: "closed" })),
  listen("unirc:disconnected", () => notify("瞬移服务器连接已断开（下次连接自动重试）")),
  listen("unirc:error", (ev) => notify("瞬移服务器错误: " + (ev.payload?.message || ""))),
]) : Promise.resolve();

// ---------- Initialize saved projects without changing saved hosts or credentials. ----------
async function init() {
  const saved = prefs.projects || {}, seenRoots = new Set();
  for (const [id, p] of Object.entries(saved)) {
    if (!p || !p.rootPath) continue;
    const norm = String(p.rootPath).replace(/\/+$/, "") || "/";
    if (seenRoots.has(norm)) continue;
    seenRoots.add(norm);
    projects.push(makeProject(id, p.name || "项目", norm, { ...p, fileFilter: p.fileFilter ?? (id === prefs.activeProjectId ? prefs.filters?.file || "" : "") }));
  }
  if (!projects.length) {
    let home = "/";
    if (inTauri) { try { home = await invoke("home_dir"); } catch (e) { toastErr(e); } }
    projects.push(makeProject("main", "默认项目", home));
  }
  activeProjectId = projects.some((p) => p.id === prefs.activeProjectId) ? prefs.activeProjectId : projects[0].id;
  setSidebarWidth(prefs.width || 300);
  loadRemoteContext(); loadProjectContext();
  renderProjects(); renderTree(); renderHosts(); renderCommandList();
  await terminalListenersReady;
  if (activeWorkspace === "remote") selectRemoteWorkspace({ reveal: false });
  else selectProject(activeProjectId, false, { reveal: false });
}
init().catch(toastErr);
