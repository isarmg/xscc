"""Fail closed when test-only mobile capabilities and package labels disagree."""
MARKER = b'XSCC_MOBILE_TEST_IP_DIRECT_V2'
LEGACY_MARKER = b'XSCC_MOBILE_TEST_LAN_DIRECT_V1'
WARNING = ('TEST ONLY: IP direct connection has no identity authentication or per-session '
           'host confirmation. Any reachable IP is allowed, with no private-address or subnet '
           'restriction. Anyone able to reach the test listener can control the host. Use a '
           'matching TEST host from the same source revision explicitly started with '
           '--test-lan-host and XSCC_TEST_LAN_BIND. Never distribute as production.')


def verify_test_capability(binary, test_lan_direct=False):
    if LEGACY_MARKER in binary or (MARKER in binary) != test_lan_direct:
        raise ValueError('Mobile test-only binary capability does not match package mode')
