#!/usr/bin/env python3
"""Check dependency ownership without compiling any platform or display driver."""
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parents[2]

def manifest(relative):
    return tomllib.loads((ROOT / relative / 'Cargo.toml').read_text())

def direct_dependencies(relative):
    data = manifest(relative)
    result = set(data.get('dependencies', {}))
    for target in data.get('target', {}).values():
        result.update(target.get('dependencies', {}))
    return result

def check():
    assert not direct_dependencies('crates/session-model'), 'Frame contract must be platform-independent'
    assert direct_dependencies('crates/native-ui') == {'gpui', 'xscc-session-model', 'unicode-segmentation'}
    assert not ({'scrap', 'librustdesk', 'rustdesk', 'gpui', 'xscc-native-ui'} & direct_dependencies('crates/session-transport'))
    for product in ('apps/desktop', 'apps/mobile'):
        assert 'xscc-native-ui' in direct_dependencies(product), product
        assert not (ROOT / product / 'Cargo.lock').exists(), product
    kvm = manifest('one-kvm/crates/management')['dependencies']
    for key, destination in [('xscc', 'crates/management'), ('xscc-native-management', 'crates/native-management')]:
        assert (ROOT / 'one-kvm/crates/management' / kvm[key]['path']).resolve() == ROOT / destination
    for retired in ('src/session_transport.rs', 'src/udp_host.rs',
                    'src/client/udp_media.rs', 'src/server/video_pipeline.rs',
                    'one-kvm/crates/management/vendor'):
        assert not (ROOT / retired).exists(), retired
    core = manifest('.')
    assert 'experimental-quic' not in core['features']
    assert core['dependencies']['xscc-session-transport']['path'] == 'crates/session-transport'
    protocol = manifest('crates/management')['dependencies']['xscs-protocol']
    assert protocol['version'] == '=4.0.0'
    assert protocol['package'] == 'xscc-management-protocol'
    assert (ROOT / 'crates/management' / protocol['path']).resolve() == ROOT / 'vendor/platform/management-protocol'
    assert protocol['default-features'] is False
    for relative in ('apps', 'crates/native-ui'):
        for source in (ROOT / relative).rglob('*.rs'):
            assert 'experiments/gpui-validation' not in source.read_text(), source
            assert 'experiments/gpui-mobile-management' not in source.read_text(), source
    print('System ownership, native products, shared identity and TCP/UDP boundaries validated.')

if __name__ == '__main__':
    check()
