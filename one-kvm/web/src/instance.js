export function instanceBase(pathname) {
  if (!pathname.startsWith("/instances/")) return "";
  const match = /^\/instances\/([a-z0-9][a-z0-9_-]{0,47})(?:\/|$)/.exec(pathname);
  if (!match) throw new Error("无效的 KVM 实例地址。");
  return `/instances/${match[1]}`;
}

export function instanceId(pathname = location.pathname) {
  return instanceBase(pathname).split("/")[2] || "default";
}

export function apiUrl(path, current = location) {
  if (!path.startsWith("/api/") || path.includes("\\")) {
    throw new Error("拒绝当前实例之外的管理请求。");
  }
  const base = instanceBase(current.pathname);
  const url = new URL(`${base}${path}`, current.origin);
  if (url.origin !== current.origin || !url.pathname.startsWith(`${base}/api/`)) {
    throw new Error("拒绝当前实例之外的管理请求。");
  }
  return url;
}

export function scopedPath(path) {
  const url = apiUrl(path);
  return `${url.pathname}${url.search}${url.hash}`;
}
