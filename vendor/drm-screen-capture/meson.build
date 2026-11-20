# SPDX-License-Identifier: MIT

project('libdrmtap', 'c',
    version: '0.5.12',
    license: 'MIT',
    default_options: [
        'c_std=c11',
        'warning_level=3',
        'werror=true',
        'b_ndebug=if-release',
    ],
)

# ============================================================================
# Version coherence
# ============================================================================
# The library version lives in three places: this project() version, the
# DRMTAP_VERSION_* macros in include/drmtap.h (returned by drmtap_version()),
# and the Rust crate version (cross-checked by the version unit tests). Parse
# the public header here and fail configuration early if it drifts from the
# project version, so the sources can never silently diverge.
fs = import('fs')
_ver_parts = {'MAJOR': '', 'MINOR': '', 'PATCH': ''}
foreach _line : fs.read('include/drmtap.h').split('\n')
    _tokens = _line.split()
    if _tokens.length() >= 3 and _tokens[0] == '#define'
        foreach _part : ['MAJOR', 'MINOR', 'PATCH']
            if _tokens[1] == 'DRMTAP_VERSION_' + _part
                _ver_parts += {_part: _tokens[2]}
            endif
        endforeach
    endif
endforeach
_hdr_version = '@0@.@1@.@2@'.format(
    _ver_parts['MAJOR'], _ver_parts['MINOR'], _ver_parts['PATCH'])
assert(_hdr_version == meson.project_version(),
    'version mismatch: meson project version is ' + meson.project_version() +
    ' but DRMTAP_VERSION_* in include/drmtap.h is ' + _hdr_version +
    ' — keep them equal')

# ============================================================================
# Dependencies
# ============================================================================

# The real floor is the FB2 GEM API (drmModeGetFB2), which landed in libdrm
# 2.4.95. HDR metadata (struct hdr_output_metadata) came in a newer libdrm than
# some bases ship, but it is optional and guarded behind HAVE_HDR_METADATA (the
# has_type probe below), so it must not raise the hard floor. Requiring 2.4.113
# needlessly broke builds on common LTS bases (e.g. ubuntu 18.04 ships 2.4.101).
libdrm_dep = dependency('libdrm', version: '>= 2.4.95')
cc = meson.get_compiler('c')
have_hdr_metadata = cc.has_type('struct hdr_output_metadata',
    prefix: '#include <xf86drmMode.h>\n#include <drm_mode.h>',
    dependencies: libdrm_dep)
if have_hdr_metadata
    add_project_arguments('-DHAVE_HDR_METADATA=1', language: 'c')
endif
threads_dep = dependency('threads')

# Optional: libva for GPU-accelerated backends
libva_dep = dependency('libva', required: false)
libva_drm_dep = dependency('libva-drm', required: false)

# Optional: EGL/GLES2 for GPU-universal detiling backend
egl_dep = dependency('egl', required: false)
glesv2_dep = dependency('glesv2', required: false)

# libseccomp (seccomp allowlist) and libcap (capability drop) confine the
# privileged helper. They are NOT optional when the helper is built — they are
# resolved as mandatory in the helper section below so an unconfined helper is
# never produced. Building the library alone does not need them.

# libm: HDR tone-mapping in pixel_convert.c uses pow() (PQ EOTF / sRGB OETF).
m_dep = meson.get_compiler('c').find_library('m', required: false)

# ============================================================================
# Library sources
# ============================================================================

lib_sources = files(
    'src/drmtap.c',
    'src/drm_enumerate.c',
    'src/drm_grab.c',
    'src/gpu_generic.c',
    'src/pixel_convert.c',
)

lib_deps = [libdrm_dep, threads_dep, m_dep]

# GPU backends: Intel and AMD (always compiled — match functions needed by drm_grab.c)
lib_sources += files(
    'src/gpu_intel.c',
    'src/gpu_amd.c',
)

# Link VAAPI if available (used by gpu_intel/gpu_amd _process functions)
if libva_dep.found() and libva_drm_dep.found()
    lib_deps += [libva_dep, libva_drm_dep]
    add_project_arguments('-DHAVE_VAAPI=1', language: 'c')
    message('VAAPI backends: enabled (Intel/AMD GPU-accelerated)')
else
    message('VAAPI backends: disabled (match-only stubs)')
endif

# Nvidia backend (no extra deps — CPU deswizzle)
lib_sources += files('src/gpu_nvidia.c')

# EGL/GLES2 GPU-universal detiling backend.
# Compile-time headers ONLY: the backend dlopen()s libEGL/libGLESv2 lazily on
# first use (see src/gpu_egl.c), so the GPU stack must never appear in the .so
# DT_NEEDED NOR in libdrmtap.pc Requires.private (a static consumer resolving
# the .pc would otherwise re-link -lEGL/-lGLESv2 into its own — possibly
# privileged — binary, defeating the whole split). We therefore do NOT attach
# egl/glesv2 as dependencies of the library target; we only pull their include
# dirs in as plain compile args. dl IS a real link dep (dlopen) and stays.
dl_dep = cc.find_library('dl', required: false)
lib_sources += files('src/gpu_egl.c')
egl_opt = get_option('egl')
have_egl = not egl_opt.disabled() \
    and egl_dep.found() and glesv2_dep.found() \
    and cc.has_header('EGL/egl.h', dependencies: egl_dep) and cc.has_header('EGL/eglext.h', dependencies: egl_dep) \
    and cc.has_header('GLES2/gl2.h', dependencies: glesv2_dep) and cc.has_header('GLES2/gl2ext.h', dependencies: glesv2_dep)
# -Degl=enabled means "this build must ship the real detiling backend": fail here
# rather than hand back a .so that looks fine and cannot detile a tiled scanout.
if egl_opt.enabled() and not have_egl
    error('egl=enabled but the egl/glesv2 pkg-config files or headers are missing '
          + '(install libegl-dev and libgles2-mesa-dev)')
endif
if have_egl
    # Add the pkg-config include dirs (if any) as compile args, without making
    # egl/glesv2 dependencies of the target — so they stay out of the .pc.
    egl_incs = run_command('pkg-config', '--cflags-only-I', 'egl', 'glesv2',
                           check: false)
    if egl_incs.returncode() == 0
        foreach _inc : egl_incs.stdout().split()
            add_project_arguments(_inc, language: 'c')
        endforeach
    endif
    if dl_dep.found()
        lib_deps += dl_dep
    endif
    add_project_arguments('-DHAVE_EGL=1', language: 'c')
    message('EGL backend: enabled (GPU-universal detiling, lazy dlopen)')
else
    message('EGL backend: disabled (stub only)')
endif

# Cursor capture
lib_sources += files('src/cursor.c')

lib_sources += files('src/privilege_helper_stub.c')

lib_include = include_directories('include', 'src')

# Export ONLY the public API from the shared object (the one the privileged
# service dlopen()s): a linker version script keeps every internal symbol
# local. Guard on GNU-ld support so a toolchain without --version-script still
# builds (it just exports the default symbol set there).
version_script = meson.current_source_dir() / 'libdrmtap.map'
libdrmtap_link_args = []
if cc.has_link_argument('-Wl,--version-script,' + version_script)
    libdrmtap_link_args += '-Wl,--version-script,' + version_script
endif

# Exploit-mitigation hardening for the shared object the privileged service
# dlopen()s — the same set the helper binary gets. Full RELRO (-z relro -z now)
# so the GOT is read-only after relocation, a stack canary, and _FORTIFY_SOURCE.
# FORTIFY needs an optimized build (at -O0, and in 'plain' where meson adds no -O
# at all, it only warns, and -Werror would then fail), so gate it on -O like the
# helper does.
libdrmtap_c_args = ['-fstack-protector-strong', '-DDRMTAP_NO_HELPER=1']
if get_option('optimization') not in ['plain', '0']
    libdrmtap_c_args += ['-U_FORTIFY_SOURCE', '-D_FORTIFY_SOURCE=2']
endif
foreach _la : ['-Wl,-z,relro', '-Wl,-z,now']
    if cc.has_link_argument(_la)
        libdrmtap_link_args += _la
    endif
endforeach

libdrmtap = shared_library('drmtap',
    lib_sources,
    include_directories: lib_include,
    dependencies: lib_deps,
    version: meson.project_version(),
    soversion: '0',
    c_args: libdrmtap_c_args,
    link_args: libdrmtap_link_args,
    link_depends: 'libdrmtap.map',
    install: true,
)

# Install the public header
install_headers('include/drmtap.h')

# pkg-config file
pkg = import('pkgconfig')
pkg.generate(libdrmtap,
    name: 'libdrmtap',
    description: 'DRM/KMS screen capture library for Linux',
    url: 'https://github.com/fxd0h/libdrmtap',
)
