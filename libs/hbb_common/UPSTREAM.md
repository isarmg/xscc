# hbb_common provenance

This directory is maintained as ordinary source in isarmg/rustdesk.

- Original repository: https://github.com/rustdesk/hbb_common
- Imported commit: `5d52530b636fb296a324260b074780cf559e0093`
- Imported from the submodule recorded by RustDesk commit `9ef277d5e`.
- Original package author: open-trade <info@opentradesolutions.com>

The imported snapshot contains no standalone license file. Existing author and
source notices are retained; this record does not introduce a new license.

Local changes remove the unused WebRTC transport, ICE signaling, coordination
and relay messages, server registration/NAT configuration, UDP proxy factories,
and their examples and dependencies. Retained protocol field numbers are not renumbered;
removed fields are reserved to prevent reuse. Configuration, LAN discovery,
permissions, encryption, TCP, and WebSocket code remain maintained here.

The historical rendezvous protocol module now contains only the signed direct
identity, connection/permission types and LAN discovery messages used by the
application. LAN discovery retains envelope field 22 and UDP port 21119.
Application-level SOCKS/HTTP proxy support and configuration have also been
removed. TCP/WebSocket connect directly, and HTTP clients explicitly disable
automatic environment proxy discovery. System network routing is unchanged.
