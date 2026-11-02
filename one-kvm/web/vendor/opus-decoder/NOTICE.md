# Opus browser decoder source materials

Production builds the unmodified ESM package sources with explicit local aliases for common, simple-yenc and the browser Worker compatibility entry. One-KVM does not load npm or a CDN at runtime. The original standalone distribution is retained as source material; it is not used as the public API because its property minification differs from the ESM package.

`archives/` retains the complete original npm tarballs; `metadata/` retains registry metadata. Every archive was checked against the registry SHA-512 integrity before extraction. `NPM-SOURCES.json` records identities, source directories and revisions. Original package contents are unchanged.

The Opus/common npm manifests declare MIT but omit an independent license file. `LICENSE-MIT-opus.txt` reproduces the standard MIT text using the original bundle copyright (2021-2025 Ethan Halsall); this supplied text is identified separately from original package contents. Original simple-yenc MIT, worker Apache-2.0 and puff source notices are retained in their original packages. `COPYING-libopus` is the upstream codec notice at revision ddbe48383984d56acd9e1ab6a090c54ca6b735a6.

The registry records opus-decoder source revision 8f2428c1cd96b54dab74836c8471ff75fe35cbee. Its upstream Git tree listing is retained for reference; it is not described as a complete Git source archive. No claim of independent implementation or upstream maintenance is made.
