#!/usr/bin/env python3
"""Generate the actual runtime inventory for a native Client installer layout."""
import argparse
import importlib.util
import json
from pathlib import Path


def inventory_module():
    spec=importlib.util.spec_from_file_location('xscc_release_inventory',Path(__file__).with_name('release-inventory.py'))
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    return module


def native_inventory(stage,runtime_path,version,revision,target,*,components=(),build_input_root=None):
    relative='share' if target.endswith('windows-msvc') else ('usr/local/share/xscc' if target.endswith('apple-darwin') else 'usr/share/doc/xscc')
    return inventory_module().generate(stage,product='xscc',version=version,revision=revision,target=target,
        root_name='xscc',runtime_files=[{'path':runtime_path,'archive_entry':None}],components=components,
        build_input_root=build_input_root,package_symlinks=True,inventory_relative=relative,
        output_relative=relative+'/release-sbom.cdx.json')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('stage','runtime-path','version','source-revision','target'):
        parser.add_argument('--'+name,required=True,type=Path if name=='stage' else str)
    parser.add_argument('--external-components',type=Path)
    parser.add_argument('--build-input-root',type=Path)
    args=parser.parse_args()
    native_inventory(args.stage,args.runtime_path,args.version,args.source_revision,args.target,
        components=json.loads(args.external_components.read_text()) if args.external_components else (),
        build_input_root=args.build_input_root)


if __name__=='__main__':main()
