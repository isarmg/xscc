import "../xcss-web/vendor/xcss-web/startup.css";
import { startApplication } from "../xcss-web/vendor/xcss-web/startup.js";
import "@xcss/web/design-tokens/tokens.css";
import "@xcss/web/design-tokens/tokens.dark.css";
import "@xcss/web/design-tokens/reset.css";
import "@xcss/web/design-tokens/accessibility.css";
import "@xcss/web/web-fonts/fonts.css";
import "./kvm.css";
import * as browser from "./media.js";
globalThis.kvmBrowser = browser;
import wasmUrl from "../pkg/one_kvm_web_bg.wasm?url";
await startApplication(async () => {
  const { default: init } = await import("../pkg/one_kvm_web.js");
  await init({ module_or_path: wasmUrl });
});
