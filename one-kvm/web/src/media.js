import { OpusDecoder } from "opus-decoder";
import { apiUrl } from "./instance.js";
export { instanceId, scopedPath } from "./instance.js";
// Browser device adapters. Views, form state, routing and HID encoding live in Rust.
let csrf = "";
let peer = null;
let channel = null;
let hidSocket = null;
let eventSocket = null;
let audioSocket = null;
let decoder = null;
let audioContext = null;
let gain = null;
let audioTime = 0;
let microphone = null;
let remoteSession = null;
let generation = 0;
let polling = null;
let computerSocket = null;
const sockets = new Set();

function uuid() {
  if (crypto.randomUUID) return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 15) | 64; bytes[8] = (bytes[8] & 63) | 128;
  return Array.from(bytes, (value, index) => `${[4,6,8,10].includes(index) ? "-" : ""}${value.toString(16).padStart(2,"0")}`).join("");
}

function announce(state, detail = "") {
  window.dispatchEvent(new CustomEvent("kvm-media", { detail: { state, detail } }));
}

function ws(path) {
  const url = apiUrl(path);
  url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
  const socket = new WebSocket(url);
  sockets.add(socket);
  socket.addEventListener("close", () => sockets.delete(socket), { once: true });
  return socket;
}

export function setCsrf(value) {
  csrf = value;
  if (!value) disconnectVideo().catch(error => announce("error", error.message));
}

function unauthorized() {
  setCsrf("");
  window.dispatchEvent(new Event("kvm-unauthorized"));
}

async function textBounded(response) {
  const reader = response.body?.getReader();
  if (!reader) return "null";
  const chunks = [];
  let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > 2 * 1024 * 1024) throw new Error("服务端响应超过大小上限。");
      chunks.push(value);
    }
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const value of chunks) { bytes.set(value, offset); offset += value.byteLength; }
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes) || "null";
}

export async function request(path, method, body) {
  const url = apiUrl(path);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), method === "PATCH" || path === "/api/setup/init" ? 60000 : 15000);
  try {
    const headers = new Headers();
    if (body !== undefined && body !== null) headers.set("Content-Type", "application/json");
    if (csrf && method !== "GET" && method !== "HEAD") headers.set("X-CSRF-Token", csrf);
    const response = await fetch(url, { method, body, headers, credentials: "same-origin", cache: "no-store", redirect: "error", signal: controller.signal });
    const text = await textBounded(response);
    if (!response.ok) {
      let message = `请求失败（${response.status}）`;
      try { const error = JSON.parse(text); message = error.message || error.error?.message || message; } catch { /* Non-JSON transport errors retain their HTTP status. */ }
      const error = new Error(message);
      error.status = response.status;
      if (response.status === 401) unauthorized();
      throw error;
    }
    return text;
  } finally {
    clearTimeout(timeout);
  }
}

async function connectMedia(mode) {
  await disconnectVideo();
  const current = generation;
  announce("connecting", "正在连接视频与输入通道…");
  await request("/api/stream/start", "POST", "{}");
  await request("/api/stream/mode", "POST", JSON.stringify({ mode }));
  if (current !== generation) return;
  hidSocket = ws("/api/ws/hid");
  hidSocket.binaryType = "arraybuffer";
  hidSocket.onmessage = event => {
    if (event.data instanceof ArrayBuffer && new Uint8Array(event.data)[0] === 1) announce("warning", "视频已连接，HID 硬件暂不可用。");
  };
  eventSocket = ws("/api/ws");
  eventSocket.onmessage = event => {
    if (typeof event.data === "string") {
      try { window.dispatchEvent(new CustomEvent("kvm-device-event", { detail: JSON.parse(event.data) })); } catch { announce("warning", "收到无效设备事件。"); }
    }
  };
  const video = document.getElementById("kvm-video");
  const image = document.getElementById("kvm-image");
  if (!video || !image) throw new Error("控制台未就绪。");
  if (mode === "mjpeg") {
    video.hidden = true;
    image.hidden = false;
    image.onload = () => announce("connected", "MJPEG · WebSocket 输入");
    image.onerror = () => announce("error", "视频流中断，请检查授权、采集设备与网络。");
    image.src = apiUrl(`/api/stream/mjpeg?client_id=${uuid()}`).href;
    return;
  }
  image.hidden = true;
  video.hidden = false;
  const ice = JSON.parse(await request("/api/webrtc/ice-servers", "GET"));
  if (current !== generation) return;
  const connection = new RTCPeerConnection({ iceServers: ice.ice_servers });
  peer = connection;
  const pending = [];
  channel = connection.createDataChannel("hid", { ordered: true, maxRetransmits: 3 });
  connection.addTransceiver("video", { direction: "recvonly" });
  connection.addTransceiver("audio", { direction: "recvonly" });
  connection.ontrack = event => {
    if (peer !== connection) return;
    const stream = video.srcObject || new MediaStream();
    stream.addTrack(event.track);
    video.srcObject = stream;
    video.play().catch(() => announce("warning", "请点击音量控件允许浏览器播放。"));
  };
  connection.onconnectionstatechange = () => {
    if (peer !== connection) return;
    const state = connection.connectionState;
    if (state === "connected") announce("connected", "WebRTC · 加密音视频与输入");
    else if (state === "failed" || state === "disconnected") announce("error", "WebRTC 已断开，请重新连接。");
  };
  connection.onicecandidate = event => {
    if (!event.candidate || peer !== connection) return;
    if (ice.mdns_mode !== "allow" && event.candidate.candidate.includes(".local")) return;
    const candidate = event.candidate.toJSON();
    if (remoteSession) request("/api/webrtc/ice", "POST", JSON.stringify({ session_id: remoteSession, candidate })).catch(error => announce("warning", error.message));
    else pending.push(candidate);
  };
  await connection.setLocalDescription(await connection.createOffer());
  const answer = JSON.parse(await request("/api/webrtc/offer", "POST", JSON.stringify({ sdp: connection.localDescription.sdp })));
  if (current !== generation) { request("/api/webrtc/close", "POST", JSON.stringify({ session_id: answer.session_id })).catch(() => {}); return; }
  remoteSession = answer.session_id;
  await connection.setRemoteDescription({ type: "answer", sdp: answer.sdp });
  for (const candidate of answer.ice_candidates || []) await connection.addIceCandidate(candidate);
  for (const candidate of pending) await request("/api/webrtc/ice", "POST", JSON.stringify({ session_id: remoteSession, candidate }));
  polling = setInterval(async () => {
    if (peer !== connection) return;
    const report = await connection.getStats();
    for (const value of report.values()) {
      if (value.type === "inbound-rtp" && value.kind === "video") announce("connected", `WebRTC · ${value.frameWidth || 0}×${value.frameHeight || 0} · ${value.framesPerSecond || 0} FPS · 丢包 ${value.packetsLost || 0}`);
    }
  }, 2000);
}

export async function connectVideo(mode) {
  try { await connectMedia(mode); }
  catch (error) { await disconnectVideo(); throw error; }
}

export function sendPacket(bytes) {
  if (channel?.readyState === "open") channel.send(bytes);
  else if (hidSocket?.readyState === WebSocket.OPEN) hidSocket.send(bytes);
  else throw new Error("输入通道尚未连接。");
}

export async function disconnectVideo() {
  generation += 1;
  const video = document.getElementById("kvm-video");
  const image = document.getElementById("kvm-image");
  if (video) { video.pause(); video.srcObject?.getTracks().forEach(track => track.stop()); video.srcObject = null; }
  if (image) { image.onload = null; image.onerror = null; image.removeAttribute("src"); }
  clearInterval(polling);
  polling = null;
  const session = remoteSession;
  remoteSession = null;
  channel?.close(); channel = null;
  peer?.close(); peer = null;
  for (const socket of sockets) socket.close();
  sockets.clear();
  hidSocket = null; eventSocket = null; audioSocket = null;
  decoder?.free(); decoder = null;
  await stopMicrophone();
  if (audioContext) { await audioContext.close(); audioContext = null; gain = null; }
  if (session && csrf) await request("/api/webrtc/close", "POST", JSON.stringify({ session_id: session })).catch(error => announce("warning", error.message));
  announce("idle", "连接已关闭。");
}


export async function setVolume(value) {
  const video = document.getElementById("kvm-video");
  if (video && !video.hidden) {
    video.muted = value === 0; video.volume = value;
    if (video.srcObject) await video.play().catch(error => announce("warning", error.message));
    return;
  }
  if (gain) { gain.gain.value = value; await audioContext.resume(); return; }
  if (!value) return;
  const current = generation;
  audioContext = new AudioContext({ sampleRate: 48000 });
  gain = audioContext.createGain(); gain.gain.value = value; gain.connect(audioContext.destination);
  try {
    await audioContext.resume();
    await request("/api/audio/start", "POST", "{}");
    if (current !== generation) return;
    const instance = new OpusDecoder({ sampleRate: 48000, channels: 2 });
    await instance.ready;
    if (current !== generation) { instance.free(); return; }
    decoder = instance;
    audioTime = audioContext.currentTime;
    audioSocket = ws("/api/ws/audio"); audioSocket.binaryType = "arraybuffer";
    audioSocket.onmessage = event => {
      if (!(event.data instanceof ArrayBuffer) || event.data.byteLength < 15 || !decoder || !audioContext) return;
      const header = new DataView(event.data);
      if (header.getUint8(0) !== 2 || header.getUint32(11, true) !== event.data.byteLength - 15) return;
      const decoded = decoder.decodeFrame(new Uint8Array(event.data, 15));
      if (decoded.errors.length) { announce("warning", decoded.errors[0].message); return; }
      if (!decoded.samplesDecoded) return;
      const buffer = audioContext.createBuffer(decoded.channelData.length, decoded.samplesDecoded, decoded.sampleRate);
      decoded.channelData.forEach((samples, index) => buffer.copyToChannel(samples, index));
      const source = audioContext.createBufferSource(); source.buffer = buffer; source.connect(gain);
      audioTime = Math.max(audioTime, audioContext.currentTime);
      if (audioTime - audioContext.currentTime > 1) audioTime = audioContext.currentTime;
      source.start(audioTime); audioTime += buffer.duration;
    };
  } catch (error) {
    if (current === generation && audioContext) { await audioContext.close(); audioContext = null; gain = null; decoder?.free(); decoder = null; }
    throw error;
  }
}

async function stopMicrophone() {
  if (!microphone) return;
  const current = microphone; microphone = null;
  current.processor.disconnect(); current.source.disconnect(); current.silence.disconnect();
  current.stream.getTracks().forEach(track => track.stop());
  current.encoder.close(); current.socket.close(); await current.context.close();
}

export async function toggleMicrophone() {
  if (microphone) { await stopMicrophone(); return false; }
  if (!isSecureContext || typeof AudioEncoder === "undefined" || typeof AudioData === "undefined") throw new Error("麦克风转发需要 HTTPS 和支持 WebCodecs 的浏览器。");
  const current = generation;
  const stream = await navigator.mediaDevices.getUserMedia({ audio: { sampleRate: 48000, channelCount: 2, echoCancellation: false, noiseSuppression: false, autoGainControl: false } });
  if (current !== generation || !csrf) { stream.getTracks().forEach(track => track.stop()); return false; }
  const socket = ws("/api/ws/uac-audio");
  const context = new AudioContext({ sampleRate: 48000 });
  const source = context.createMediaStreamSource(stream);
  const processor = context.createScriptProcessor(1024, 2, 2);
  const silence = context.createGain(); silence.gain.value = 0;
  source.connect(processor); processor.connect(silence); silence.connect(context.destination);
  let timestamp = 0;
  const encoder = new AudioEncoder({ output(chunk) {
    if (socket.readyState !== WebSocket.OPEN || socket.bufferedAmount > 256 * 1024) return;
    const bytes = new Uint8Array(15 + chunk.byteLength); const header = new DataView(bytes.buffer);
    header.setUint8(0, 3); header.setUint32(1, Math.round(chunk.timestamp / 1000), true); header.setUint16(5, Math.round((chunk.duration || 0) / 1000), true); header.setUint32(7, 0, true); header.setUint32(11, chunk.byteLength, true); chunk.copyTo(bytes.subarray(15)); socket.send(bytes);
  }, error(error) { announce("error", error.message); stopMicrophone(); } });
  encoder.configure({ codec: "opus", sampleRate: 48000, numberOfChannels: 2, bitrate: 64000 });
  processor.onaudioprocess = event => {
    if (encoder.encodeQueueSize >= 4 || socket.readyState !== WebSocket.OPEN) return;
    const count = event.inputBuffer.length;
    const samples = new Float32Array(count * 2);
    samples.set(event.inputBuffer.getChannelData(0)); samples.set(event.inputBuffer.getChannelData(Math.min(1, event.inputBuffer.numberOfChannels - 1)), count);
    const data = new AudioData({ format: "f32-planar", sampleRate: 48000, numberOfFrames: count, numberOfChannels: 2, timestamp, data: samples });
    encoder.encode(data); data.close(); timestamp += count / 48000 * 1000000;
  };
  microphone = { stream, socket, context, source, processor, silence, encoder };
  socket.onclose = () => { if (microphone?.socket === socket) { announce("warning", "麦克风连接已关闭。"); stopMicrophone(); } };
  await context.resume();
  return true;
}

export function upload(path, inputId) {
  const file = document.getElementById(inputId)?.files?.[0];
  if (!file) return Promise.reject(new Error("请先选择文件。"));
  return new Promise((resolve, reject) => {
    const form = new FormData(); form.append("file", file);
    const request = new XMLHttpRequest();
    request.open("POST", apiUrl(path)); request.withCredentials = true; request.setRequestHeader("X-CSRF-Token", csrf);
    request.upload.onprogress = event => window.dispatchEvent(new CustomEvent("kvm-upload", { detail: event.lengthComputable ? Math.round(event.loaded / event.total * 100) : 0 }));
    request.onerror = () => reject(new Error("上传连接中断。"));
    request.onload = () => {
      if (request.status === 401) unauthorized();
      if (request.status >= 200 && request.status < 300) resolve(request.responseText);
      else reject(new Error(`上传失败（${request.status}）。`));
    };
    request.send(form);
  });
}

export function fullscreen() { return document.getElementById("kvm-viewport")?.requestFullscreen(); }
export function codecSupported(value) {
  if (value === "mjpeg") return true;
  const codecs = globalThis.RTCRtpReceiver?.getCapabilities?.("video")?.codecs || [];
  return codecs.some(codec => codec.mimeType.toLowerCase() === `video/${value}`.toLowerCase());
}
export function toggleTheme() { const html = document.documentElement; html.setAttribute("data-theme", html.getAttribute("data-theme") === "dark" ? "light" : "dark"); }
export function stopComputerSocket() { computerSocket?.close(); computerSocket = null; }
export async function startComputerUse(prompt, continue_conversation) {
  stopComputerSocket();
  const clientId = uuid();
  const socket = ws(`/api/ws/computer-use?client_id=${encodeURIComponent(clientId)}`);
  computerSocket = socket;
  socket.onmessage = async event => {
    if (typeof event.data !== "string" || event.data.length > 8 * 1024 * 1024) return;
    let message;
    try { message = JSON.parse(event.data); } catch { announce("error", "收到无效自动操作消息。"); return; }
    window.dispatchEvent(new CustomEvent("kvm-computer-use", { detail: event.data }));
    if (message.type !== "screenshot_requested") return;
    try {
      const video = document.getElementById("kvm-video");
      const image = document.getElementById("kvm-image");
      const source = image?.hidden ? video : image;
      const width = source === video ? video.videoWidth : image.naturalWidth;
      const height = source === video ? video.videoHeight : image.naturalHeight;
      if (!width || !height) throw new Error("画面尚未就绪，请先连接视频。");
      const canvas = document.createElement("canvas"); canvas.width = width; canvas.height = height;
      canvas.getContext("2d").drawImage(source, 0, 0);
      const screenshot = { data_url: canvas.toDataURL("image/jpeg", 0.82), width, height };
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "screenshot_result", request_id: message.request_id, screenshot }));
    } catch (error) {
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "screenshot_error", request_id: message.request_id, message: error.message }));
    }
  };
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { socket.close(); reject(new Error("自动操作连接超时。")); }, 8000);
    socket.onopen = () => { clearTimeout(timeout); resolve(); };
    socket.onerror = () => { clearTimeout(timeout); reject(new Error("自动操作连接失败。")); };
  });
  return request("/api/computer-use/session", "POST", JSON.stringify({ prompt, continue_conversation, client_id: clientId }));
}
window.addEventListener("pagehide", () => { csrf = ""; disconnectVideo().catch(() => {}); });
window.addEventListener("blur", () => { if (csrf && hidSocket?.readyState === WebSocket.OPEN) request("/api/hid/reset", "POST", "{}").catch(error => announce("warning", error.message)); });
