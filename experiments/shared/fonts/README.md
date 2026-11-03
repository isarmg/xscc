# Client Maple fonts

These are the user's full Sarmg Maple fonts already vendored under
`one-kvm/web/vendor/xcss/web-fonts/MapleMonoFull-{Regular,Bold}.woff2`.
Only the WOFF2 container was unpacked to native TrueType using FontTools 4.57.0;
glyphs, character coverage, horizontal metrics, names and weights are retained.
The internal native family is `Maple Mono Normal NL`, with static 400 and 700
faces and complete user-selected CJK coverage. Do not substitute the Latin-only
upstream variable font or the web CSS alias `Sarmg Maple`.

Desktop and Apple clients register the embedded TTFs before opening any window.
Android packages these same TTFs as assets and loads each once for GPUI and the
native editor. The fonts are distributed under the adjacent SIL OFL 1.1 notice.

