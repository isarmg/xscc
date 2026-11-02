// Browser acceptance against explicit local API/media fixtures, not KVM hardware.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { readFile, writeFile } from "node:fs/promises";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
const web = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(web, "dist");
const instancePrefix = "/instances/rack-2";
const sessionCookie = "one_kvm_session_rack-2";
const { chromium, firefox } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const image = await readFile(resolve(web, "public/favicon.png"));
const code = "a".repeat(36);
const csrf = "fixture-csrf-token";
const controller = { manager_id: "11111111-1111-4111-8111-111111111111", device_id: "22222222-2222-4222-8222-222222222222", installation_id: "33333333-3333-4333-8333-333333333333" };
const controlled = { ...controller, device_id: "44444444-4444-4444-8444-444444444444", installation_id: "55555555-5555-4555-8555-555555555555" };
let state;
let base;
let rtcPage;
let rtcReady;
const requests = [];
const sockets = new Set();
const streams = new Set();
const hid = [];
const result = [];
const skipWebRtc = process.env.KVM_BROWSER_SKIP_WEBRTC === "1";
const skipAudio = process.env.KVM_BROWSER_SKIP_AUDIO === "1";
const browserEngines = new Set((process.env.KVM_BROWSER_ENGINES || "chromium,firefox").split(","));
assert([...browserEngines].every(name => name === "chromium" || name === "firefox"));
assert(browserEngines.size > 0);
function reset(mode) {
  state = { mode, configured: true, authenticated: false, initialized: false, audio: { enabled: true, device: "fixture-microphone", quality: "balanced" }, video: { device: mode === "windows" ? "desktop:DISPLAY1" : "/dev/video0", format: "MJPEG", width: 1920, height: 1080, fps: 30, quality: 80 }, addresses: [{ host: "10.0.0.10", port: 21118 }] };
  requests.length = 0; hid.length = 0;
}
function session() { return { authenticated: true, csrf_token: csrf, expires_in_secs: 3600, controller, controlled }; }
function json(response, value, status = 200) { response.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store" }); response.end(JSON.stringify(value)); }
function authorized(request) { return state.authenticated && (request.headers.cookie || "").includes(`${sessionCookie}=fixture`); }
const server = createServer(async (request, response) => {
  const url = new URL(request.url, base || "http://127.0.0.1");
  const mounted = url.pathname.startsWith(`${instancePrefix}/`);
  const localPath = mounted ? url.pathname.slice(instancePrefix.length) : url.pathname;
  try {
    if (url.pathname === "/instances/other/api/info") {
      assert(!(request.headers.cookie || "").includes(`${sessionCookie}=`));
      return json(response, { message: "other instance is not authenticated" }, 401);
    }
    if (!mounted && url.pathname !== "/rtc-fixture") return response.writeHead(404).end();
    if (localPath.startsWith("/api/")) {
      const data = [];
      for await (const chunk of request) data.push(chunk);
      const body = Buffer.concat(data).toString();
      requests.push({ path: localPath, rawPath: url.pathname, method: request.method, body, headers: request.headers });
      if (localPath === "/api/management/status") return json(response, { configured: state.configured, phase: "online" });
      if (localPath === "/api/management/login") {
        assert.equal(request.headers.origin, base);
        assert.equal(JSON.parse(body).authorization_code, code);
        state.authenticated = true;
        response.setHeader("Set-Cookie", `${sessionCookie}=fixture; HttpOnly; SameSite=Strict; Path=${instancePrefix}/`);
        return json(response, session());
      }
      if (!authorized(request)) return json(response, { message: "fixture authorization expired" }, 401);
      if (!["GET", "HEAD"].includes(request.method)) assert.equal(request.headers["x-csrf-token"], csrf);
      const path = localPath;
      if (path === "/api/auth/check") return json(response, session());
      if (path === "/api/auth/logout") { state.authenticated = false; return json(response, { success: true }); }
      if (path === "/api/setup") return json(response, { initialized: state.initialized, needs_setup: !state.initialized, platform: { mode: state.mode, msd: { available: true }, otg: { available: true }, hid: { backends: state.mode === "windows" ? ["none", "ch9329", "windows"] : ["none", "otg", "ch9329", "bluetooth"] } } });
      if (path === "/api/setup/init") { const selected = JSON.parse(body); if (selected.video_device) state.video.device = selected.video_device; state.initialized = true; return json(response, { success: true }); }
      if (path === "/api/devices") return json(response, { video: [{ path: "/dev/video0", name: "Fixture HDMI" }, ...(state.mode === "windows" ? [{ path: "desktop:DISPLAY1", name: "Display 1" }, { path: "desktop:DISPLAY2", name: "Display 2" }] : [])], serial: [{ path: "/dev/ttyUSB0", name: "Fixture CH9329" }], audio: [{ name: "fixture-microphone", description: "Fixture microphone" }, { name: "fixture-capture-audio", description: "Fixture capture audio" }], udc: [] });
      if (path === "/api/management/device") return json(response, { addresses: state.addresses });
      if (path === "/api/management/addresses") { state.addresses = JSON.parse(body).addresses; return json(response, { success: true }); }
      if (path === "/api/config/rustdesk") return json(response, request.method === "GET" ? { enabled: true, mode: "direct_ip", codec: "h264", direct_access_port: 21118 } : { success: true });
      if (path === "/api/config/video") { if (request.method === "PATCH") Object.assign(state.video, JSON.parse(body)); return json(response, state.video); }
      if (path === "/api/config/audio") { if (request.method === "PATCH") Object.assign(state.audio, JSON.parse(body)); return json(response, state.audio); }
      if (path === "/api/stream/codecs") return json(response, { codecs: [{ id: "mjpeg", name: "MJPEG", available: true }, { id: "vp8", name: "VP8", available: true }, { id: "h264", name: "H.264", available: true }] });
      if (path === "/api/stream/status") return json(response, { state: "streaming", device: "/dev/video0", fps: 30, clients: 1, resolution: [1920, 1080] });
      if (path === "/api/stream/constraints") return json(response, { success: true, allowed_codecs: ["vp8", "h264"], locked_codec: null, disallow_mjpeg: false, sources: { rustdesk: false, rtsp: false, vnc: false }, reason: "", current_mode: "mjpeg" });
      if (path === "/api/webrtc/ice-servers") return json(response, { ice_servers: [], mdns_mode: "allow" });
      if (path === "/api/webrtc/offer") {
        rtcReady = rtcPage.evaluate(async ({ sdp }) => {
          const peer = new RTCPeerConnection({ iceServers: [] });
          globalThis.__fixturePeer = peer; globalThis.__fixtureHid = [];
          peer.ondatachannel = event => { event.channel.binaryType = "arraybuffer"; event.channel.onmessage = event => globalThis.__fixtureHid.push([...new Uint8Array(event.data)]); };
          const canvas = document.createElement("canvas"); canvas.width = 320; canvas.height = 180;
          const context = canvas.getContext("2d"); let frame = 0;
          globalThis.__fixtureDraw = setInterval(() => { context.fillStyle = frame++ % 2 ? "#0f766e" : "#164e63"; context.fillRect(0,0,320,180); }, 100);
          const stream = canvas.captureStream(10); globalThis.__fixtureStream = stream;
          peer.addTrack(stream.getVideoTracks()[0], stream);
          await peer.setRemoteDescription({ type: "offer", sdp });
          await peer.setLocalDescription(await peer.createAnswer());
          if (peer.iceGatheringState !== "complete") await new Promise(resolve => { peer.onicegatheringstatechange = () => { if (peer.iceGatheringState === "complete") resolve(); }; });
          return { session_id: "fixture-rtc-session", sdp: peer.localDescription.sdp, ice_candidates: [] };
        }, JSON.parse(body));
        return json(response, await rtcReady);
      }
      if (path === "/api/webrtc/ice") { await rtcReady; await rtcPage.evaluate(candidate => globalThis.__fixturePeer.addIceCandidate(candidate), JSON.parse(body).candidate); return json(response, { success: true }); }
      if (path === "/api/webrtc/close") { await rtcPage.evaluate(() => { globalThis.__fixturePeer?.close(); globalThis.__fixtureStream?.getTracks().forEach(track => track.stop()); clearInterval(globalThis.__fixtureDraw); }); return json(response, { success: true }); }
      if (path === "/api/stream/mjpeg") {
        response.writeHead(200, { "Content-Type": "multipart/x-mixed-replace; boundary=frame", "Cache-Control": "no-store" });
        streams.add(response);
        const frame = () => { response.write(`--frame\r\nContent-Type: image/png\r\nContent-Length: ${image.length}\r\n\r\n`); response.write(image); response.write("\r\n"); };
        frame(); const timer = setInterval(frame, 250);
        response.on("close", () => { clearInterval(timer); streams.delete(response); });
        return;
      }
      if (path === "/api/msd/images") return json(response, request.method === "GET" ? [{ id: "image-1", name: "Fixture ISO", size: 1048576 }] : { id: "uploaded", name: "fixture.iso", size: body.length });
      if (path === "/api/msd/status") return json(response, { available: true, state: { disk_mode: "single", slot_capacity: 1, mounted_count: 0, mounted_media: [], usb_reenumerating: false } });
      if (path === "/api/msd/drive/files") return json(response, []);
      if (path === "/api/computer-use/session") return json(response, { status: "idle", step: 0, last_error: null, final_message: null });
      if (path === "/api/terminal/") { response.writeHead(200, { "Content-Type": "text/html" }); response.end("<!doctype html><p>受管终端 API 替身</p>"); return; }
      if (path === "/api/info") return json(response, { version: "fixture", device_info: null });
      return json(response, { success: true });
    }
    if (url.pathname === "/rtc-fixture") { response.writeHead(200, { "Content-Type": "text/html" }); response.end("<!doctype html><title>Browser WebRTC media fixture</title>"); return; }
    const pathname = decodeURIComponent(localPath);
    let filename = resolve(dist, `.${pathname}`);
    if (!filename.startsWith(`${dist}/`) && filename !== dist) return response.writeHead(403).end();
    if (!extname(pathname)) filename = resolve(dist, "index.html");
    const bytes = await readFile(filename);
    const mime = { ".html": "text/html", ".js": "application/javascript", ".wasm": "application/wasm", ".css": "text/css", ".woff2": "font/woff2", ".png": "image/png", ".json": "application/json", ".txt": "text/plain" }[extname(filename)] || "application/octet-stream";
    response.writeHead(200, { "Content-Type": mime, "X-Content-Type-Options": "nosniff", "Cache-Control": "no-store", "Content-Security-Policy": "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; font-src 'self'; connect-src 'self' ws:; img-src 'self' data:; frame-src 'self'; worker-src 'self' blob:" });
    response.end(bytes);
  } catch (error) { json(response, { message: error.message }, 500); }
});
server.on("upgrade", (request, socket) => {
  if (!request.url.startsWith(`${instancePrefix}/api/`)) { socket.end("HTTP/1.1 404 Not Found\r\n\r\n"); return; }
  const localPath = request.url.slice(instancePrefix.length);
  if (!authorized(request) || request.headers.origin !== base) { socket.end("HTTP/1.1 401 Unauthorized\r\n\r\n"); return; }
  const accept = createHash("sha1").update(request.headers["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
  socket.fixturePath = request.url; sockets.add(socket); socket.on("close", () => sockets.delete(socket)); socket.on("error", () => sockets.delete(socket));
  if (localPath === "/api/ws/hid") socket.write(Buffer.from([0x82, 1, 0]));
  if (localPath === "/api/ws/audio") {
    const packet = Buffer.alloc(18); packet[0] = 2; packet.writeUInt16LE(20, 5); packet.writeUInt32LE(3, 11); packet.set([0xf8,0xff,0xfe],15);
    socket.write(Buffer.concat([Buffer.from([0x82,packet.length]),packet]));
  }
  let pending = Buffer.alloc(0);
  socket.on("data", data => {
    pending = Buffer.concat([pending,data]);
    while (pending.length >= 2) {
      const opcode = pending[0] & 15; const masked = (pending[1] & 128) !== 0;
      let length = pending[1] & 127, offset = 2;
      if (length === 126) { if (pending.length < 4) return; length = pending.readUInt16BE(2); offset = 4; }
      if (length === 127) { socket.destroy(); return; }
      const mask = masked ? pending.subarray(offset,offset+4) : null;
      if (masked) offset += 4;
      if (pending.length < offset+length) return;
      const payload = Buffer.from(pending.subarray(offset,offset+length));
      if (mask) for (let index = 0; index < payload.length; index++) payload[index] ^= mask[index%4];
      pending = pending.subarray(offset+length);
      if (opcode === 8) { socket.end(Buffer.from([0x88,0])); return; }
      if (opcode === 9) { socket.write(Buffer.concat([Buffer.from([0x8a,payload.length]),payload])); continue; }
      if (localPath === "/api/ws/hid" && opcode === 2) hid.push([...payload]);
    }
  });
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
base = `http://127.0.0.1:${server.address().port}`;
const wait = async (predicate, label) => { for (let count = 0; count < 100; count++) { if (predicate()) return; await new Promise(resolve => setTimeout(resolve, 50)); } throw new Error(`Timed out: ${label}`); };
try {
  for (const [name, engine] of [["chromium", chromium], ["firefox", firefox]]) {
    if (!browserEngines.has(name)) continue;
    reset(name === "chromium" ? "windows" : "linux");
    const executablePath = name === "chromium" ? process.env.CHROMIUM_BIN : process.env.FIREFOX_BIN;
    const browser = await engine.launch({ headless: true, ...(executablePath ? { executablePath } : {}) });
    const page = await browser.newPage({ viewport: { width: 1440, height: 1050 } });
    rtcPage = await browser.newPage(); await rtcPage.goto(`${base}/rtc-fixture`);
    const failures = []; const wasm = [];
    await page.addInitScript(() => {
      globalThis.__audioStarts = 0;
      const original = AudioBufferSourceNode.prototype.start;
      AudioBufferSourceNode.prototype.start = function(...args) { globalThis.__audioStarts++; return original.apply(this,args); };
    });
    page.on("pageerror", error => failures.push(error.message));
    page.on("response", response => { if (response.url().endsWith(".wasm")) wasm.push({ url: response.url(), status: response.status(), mime: response.headers()["content-type"] }); });
    try {
      await page.goto(`${base}${instancePrefix}/`);
      await page.waitForSelector('html[data-xcss-runtime="xcss-web"]', { timeout: 30000 });
      await page.getByLabel("控制端实例授权码").fill(code);
      await page.getByRole("button", { name: "验证授权并连接", exact: true }).click();
      await page.getByRole("heading", { name: "初始化 KVM 硬件", exact: true }).waitFor();
      if (state.mode === "windows") {
        await page.getByLabel("显示器 / 视频采集设备", { exact: true }).selectOption("desktop:DISPLAY2");
        await page.getByLabel("HID 后端", { exact: true }).selectOption("windows");
        await page.getByLabel("Windows 输入映射显示器", { exact: true }).selectOption("desktop:DISPLAY2");
        await page.getByLabel("Windows 键盘输入", { exact: true }).uncheck();
        await page.getByRole("button", { name: "保存选择并初始化", exact: true }).click();
      } else {
        await page.getByRole("button", { name: "初始化并保留当前硬件配置", exact: true }).click();
      }
      await page.getByRole("heading", { name: "远程控制台", exact: true }).waitFor();
      assert(requests.some(value => value.path === "/api/setup/init" && value.headers["x-csrf-token"] === csrf));
      const setup = JSON.parse(requests.find(value => value.path === "/api/setup/init").body);
      if (state.mode === "windows") assert.deepEqual(setup, { video_device: "desktop:DISPLAY2", hid_backend: "windows", hid_windows_monitor: "desktop:DISPLAY2", hid_windows_keyboard: false, hid_windows_mouse: true });
      else assert.deepEqual(setup, {});
      assert(await page.evaluate(value => !JSON.stringify(localStorage).includes(value), code));
      await page.getByRole("link", { name: "设备设置", exact: true }).click();
      await page.getByRole("button", { name: "音频采集", exact: true }).click();
      await page.getByLabel("音频设备名称", { exact: true }).selectOption("fixture-capture-audio");
      await page.getByRole("button", { name: "保存设置", exact: true }).click();
      await wait(() => state.audio.device === "fixture-capture-audio", "instance audio selection");
      assert.deepEqual(JSON.parse(requests.find(value => value.path === "/api/config/audio" && value.method === "PATCH").body), { device: "fixture-capture-audio" });
      await page.getByRole("button", { name: "视频采集", exact: true }).click();
      await page.getByLabel("显示器 / 视频采集设备", { exact: true }).selectOption("/dev/video0");
      await page.getByRole("button", { name: "保存设置", exact: true }).click();
      await wait(() => requests.some(value => value.path === "/api/config/video" && value.method === "PATCH"), "instance video selection");
      assert.deepEqual(JSON.parse(requests.find(value => value.path === "/api/config/video" && value.method === "PATCH").body), { device: "/dev/video0" });
      await page.getByRole("button", { name: "原生 XSCC", exact: true }).click();
      await page.getByRole("button", { name: "保存设置", exact: true }).waitFor({ state: "visible" });
      await wait(() => requests.some(value => value.path === "/api/config/rustdesk" && value.method === "GET"), "native settings load");
      await page.getByLabel("原生监听端口", { exact: true }).fill("21119");
      await page.getByRole("button", { name: "保存设置", exact: true }).click();
      await wait(() => requests.some(value => value.path === "/api/config/rustdesk" && value.method === "PATCH"), "native settings save");
      const native = JSON.parse(requests.find(value => value.path === "/api/config/rustdesk" && value.method === "PATCH").body);
      assert.equal(native.enabled, true); assert.equal(native.mode, "direct_ip"); assert.equal(native.direct_access_port, 21119); assert(!("device_password" in native));
      await page.getByLabel("地址 1", { exact: true }).fill("kvm.example.test");
      await page.getByLabel("外部可达端口", { exact: true }).first().fill("24443");
      await page.getByRole("button", { name: "保存上报地址", exact: true }).click();
      await wait(() => state.addresses[0]?.host === "kvm.example.test", "reported addresses");
      assert.equal(state.addresses[0].port, 24443);
      await page.getByRole("link", { name: "虚拟媒体", exact: true }).click();
      await page.locator("#kvm-image-upload").setInputFiles({ name: "fixture.iso", mimeType: "application/octet-stream", buffer: Buffer.from("fixture ISO data") });
      await page.getByRole("button", { name: "上传镜像", exact: true }).click();
      await wait(() => requests.some(value => value.path === "/api/msd/images" && value.method === "POST" && value.body.includes("fixture ISO data")), "authenticated upload");
      await page.getByRole("link", { name: "控制台", exact: true }).click();
      let rtc = { skipped: "explicitly disabled by KVM_BROWSER_SKIP_WEBRTC=1" };
      if (!skipWebRtc) {
      await page.getByLabel("视频传输方式").selectOption("vp8");
      await page.getByRole("button", { name: "连接画面", exact: true }).click();
      await page.waitForFunction(() => { const video = document.getElementById("kvm-video"); return video?.videoWidth === 320 && video.readyState >= 2; });
      await page.locator("#kvm-viewport").focus(); await page.keyboard.press("b");
      await rtcPage.waitForFunction(() => globalThis.__fixtureHid.some(value => value[0] === 1 && value[2] === 5));
      rtc = { video_width: await page.locator("#kvm-video").evaluate(video => video.videoWidth), transport: "real browser loopback RTCPeerConnection", hid: "real RTCDataChannel" };
      await page.getByRole("button", { name: "断开", exact: true }).click();
      await rtcPage.waitForFunction(() => globalThis.__fixturePeer.connectionState === "closed");
      }
      await page.getByLabel("视频传输方式").selectOption("mjpeg");
      await page.getByRole("button", { name: "连接画面", exact: true }).click();
      await wait(() => sockets.size >= 2 && streams.size > 0, "fixture media and input channels");
      await page.locator("#kvm-viewport").focus(); await page.keyboard.press("a");
      await wait(() => hid.some(value => value[0] === 1 && value[1] === 0 && value[2] === 4), "physical HID packet");
      let decoded = { skipped: "explicitly disabled by KVM_BROWSER_SKIP_AUDIO=1" };
      if (!skipAudio) {
      await page.locator("input[type=range]").click();
      assert(Number(await page.locator("input[type=range]").inputValue()) > 0);
      await page.waitForFunction(() => globalThis.__audioStarts > 0).catch(async error => { console.error("Audio fixture diagnostics", await page.locator(".error").allTextContents(), [...sockets].length, requests.map(value=>value.path)); throw error; });
      decoded = { audio_buffer_starts: await page.evaluate(() => globalThis.__audioStarts), input: "valid Opus silence packet", decoder: "pinned original local ESM sources" };
      }
      state.authenticated = false;
      await page.evaluate(() => globalThis.kvmBrowser.request("/api/info", "GET").catch(() => {}));
      await page.getByRole("heading", { name: "连接你的 KVM", exact: true }).waitFor();
      await wait(() => sockets.size === 0 && streams.size === 0, "401 media cleanup").catch(error => { console.error("cleanup fixture diagnostics", [...sockets].map(value=>value.fixturePath), streams.size); throw error; });
      await page.getByLabel("控制端实例授权码").fill(code);
      await page.getByRole("button", { name: "验证授权并连接", exact: true }).click();
      await page.getByRole("heading", { name: "远程控制台", exact: true }).waitFor();
      await page.getByLabel("视频传输方式").selectOption("mjpeg");
      await page.getByRole("button", { name: "连接画面", exact: true }).click();
      await wait(() => sockets.size >= 2 && streams.size > 0, "reconnected media");
      const lateMicrophone = await page.evaluate(() => {
        if (typeof AudioEncoder === "undefined" || typeof AudioData === "undefined") return false;
        globalThis.__fixtureAudio = new AudioContext();
        globalThis.__fixtureLateStream = globalThis.__fixtureAudio.createMediaStreamDestination().stream;
        globalThis.__fixtureOriginalMic = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
        navigator.mediaDevices.getUserMedia = () => new Promise(resolve => { globalThis.__fixtureResolveMic = resolve; });
        globalThis.__fixturePendingMic = globalThis.kvmBrowser.toggleMicrophone(); return true;
      });
      await page.getByRole("button", { name: "退出授权会话", exact: true }).click();
      await page.getByRole("heading", { name: "连接你的 KVM", exact: true }).waitFor();
      await wait(() => sockets.size === 0 && streams.size === 0, "logout media cleanup");
      if (lateMicrophone) {
        assert.equal(await page.evaluate(async () => { globalThis.__fixtureResolveMic(globalThis.__fixtureLateStream); const active = await globalThis.__fixturePendingMic; const ended = globalThis.__fixtureLateStream.getTracks().every(track => track.readyState === "ended"); navigator.mediaDevices.getUserMedia = globalThis.__fixtureOriginalMic; await globalThis.__fixtureAudio.close(); return !active && ended; }), true);
        assert.equal(sockets.size, 0);
      }
      await page.getByLabel("控制端实例授权码").fill(code);
      await page.getByRole("button", { name: "验证授权并连接", exact: true }).click();
      await page.getByRole("heading", { name: "远程控制台", exact: true }).waitFor();
      await page.getByLabel("视频传输方式").selectOption("mjpeg");
      await page.getByRole("button", { name: "连接画面", exact: true }).click();
      await wait(() => sockets.size >= 2 && streams.size > 0, "media before upload authorization expires");
      state.authenticated = false;
      await page.evaluate(async () => {
        const input = document.createElement("input"); input.type = "file"; input.id = "fixture-unauthorized-upload";
        const data = new DataTransfer(); data.items.add(new File(["fixture ISO"], "expired.iso")); input.files = data.files; document.body.append(input);
        try { await globalThis.kvmBrowser.upload("/api/msd/images", input.id); } catch { /* The expected 401 must revoke the UI and all media. */ } finally { input.remove(); }
      });
      await page.getByRole("heading", { name: "连接你的 KVM", exact: true }).waitFor();
      await wait(() => sockets.size === 0 && streams.size === 0, "upload 401 media cleanup");
      assert(wasm.some(value => value.status === 200 && value.mime === "application/wasm"));
      assert(wasm.every(value => new URL(value.url).pathname.startsWith(`${instancePrefix}/`)));
      assert(requests.every(value => value.rawPath.startsWith(`${instancePrefix}/api/`)));
      assert.equal(await page.getByRole("link", { name: "切换实例", exact: true }).getAttribute("href"), "/");
      assert.equal(await page.evaluate(() => fetch("/instances/other/api/info").then(response => response.status)), 401);
      assert.equal(await page.evaluate(async () => { try { await globalThis.kvmBrowser.request("/instances/other/api/info", "GET"); return false; } catch { return true; } }), true);
      assert.equal(await page.evaluate(() => document.querySelector("[data-v-app]") !== null), false);
      assert.deepEqual(failures, []);
      result.push({ browser: name, version: browser.version(), passed: true, wasm, webrtc: rtc, opus: decoded, late_microphone_cancelled: lateMicrophone ? true : "WebCodecs encoder unavailable", exercised: ["instance-prefixed WASM/assets/API/media/upload/navigation", "instance cookie boundary", "Rust WASM mount", "controller login and CSRF", "authorized hardware initialization", ...(state.mode === "windows" ? ["Windows display and per-role input selection"] : []), "instance video/audio assignment", "managed native settings", "NAT addresses", "authenticated multipart upload", ...(skipWebRtc ? [] : ["WebRTC frame decode and HID data channel"]), "MJPEG and HID encoding", ...(skipAudio ? [] : ["local Opus decode"]), "fetch/upload 401 and logout media cancellation"] });
    } catch (error) {
      console.error("Browser fixture diagnostics", { failures, labels: await page.locator("label").allTextContents(), messages: await page.locator(".error, .status-line").allTextContents(), requests: requests.slice(-24).map(value => ({ path: value.rawPath, method: value.method })) });
      throw error;
    } finally { await browser.close(); for (const socket of sockets) socket.destroy(); for (const stream of streams) stream.end(); }
  }
  await writeFile(process.env.BROWSER_EVIDENCE || "/tmp/one-kvm-web-browser.json", JSON.stringify({ scope: "real browsers and current WASM; API/media fixtures; no KVM hardware acceptance", results: result }, null, 2));
  console.log(JSON.stringify(result));
} finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
