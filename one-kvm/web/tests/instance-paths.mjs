import assert from "node:assert/strict";
import test from "node:test";
import { apiUrl, instanceBase, instanceId } from "../src/instance.js";

test("API, media and upload paths remain within the selected instance", () => {
  for (const instance of ["default", "rack-2", "room_3"]) {
    const current = new URL(`https://kvm.example/instances/${instance}/#settings`);
    assert.equal(instanceId(current.pathname), instance);
    for (const path of ["/api/config/video", "/api/ws/hid", "/api/webrtc/offer", "/api/terminal/", "/api/msd/images", "/api/stream/mjpeg?client_id=test"]) {
      assert.equal(apiUrl(path, current).href, `https://kvm.example/instances/${instance}${path}`);
    }
    assert.equal(apiUrl("/api/msd/drive/files?path=%2Fimages%2Fboot.iso", current).searchParams.get("path"), "/images/boot.iso");
  }
});

test("management requests cannot escape the current instance or origin", () => {
  const current = new URL("https://kvm.example/instances/rack-2/");
  for (const path of ["https://other.example/api/info", "//other.example/api/info", "/instances/default/api/info", "/api/../../default/api/info", "/api/%2e%2e/config/video", "/api/..\\default/api/info"]) {
    assert.throws(() => apiUrl(path, current));
  }
  for (const path of ["/instances/../default/", "/instances/A/", "/instances/%2e%2e/", `/instances/${"a".repeat(49)}/`]) {
    assert.throws(() => instanceBase(path));
  }
});

test("the development root keeps its same-origin API paths", () => {
  assert.equal(instanceId("/"), "default");
  assert.equal(apiUrl("/api/info", new URL("http://127.0.0.1:8080/#system")).href, "http://127.0.0.1:8080/api/info");
});
