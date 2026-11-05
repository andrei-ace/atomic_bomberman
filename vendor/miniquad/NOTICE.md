# Vendored Miniquad

Source: miniquad 0.4.11, https://github.com/not-fl3/miniquad
Original crate SHA-256: `dcbaea978d741f3b60ced666b06a2a1e4a2bc81cb8b9dd29f4a40965e1f59e65`.

The Linux X11 fullscreen implementation uses standard window-manager ADD/REMOVE
requests for a mapped window, preserving keyboard focus and avoiding GNOME's
unmap/remap race. Initial fullscreen uses the normal pre-map property hint.
The Metal pipeline honors a separately configured `alpha_blend`, falling back
to the RGB blend state only when no alpha state is supplied. This lets normal
source-over rendering preserve opaque destination coverage. A duplicate RGB
blend-factor setter was removed from the same pipeline setup.

All other upstream source is unchanged. Original MIT and Apache licenses are
included.
