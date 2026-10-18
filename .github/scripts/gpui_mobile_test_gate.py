"""Fail closed when test-only mobile capabilities and package labels disagree."""
MARKER = b'XSCC_MOBILE_TEST_LAN_DIRECT_V1'
WARNING = ('TEST ONLY: LAN direct connection has no identity authentication or per-session '
           'host confirmation. Use only on an isolated test LAN with a matching host explicitly '
           'started with --test-lan-host and XSCC_TEST_LAN_BIND. Never distribute as production.')


def verify_test_capability(binary, test_lan_direct=False):
    if (MARKER in binary) != test_lan_direct:
        raise ValueError('Mobile test-only binary capability does not match package mode')
