# Metal shader sources

Compiled at runtime with `MTLDevice.makeLibrary(source:)` (no build-time `metal`
compiler is available with Command Line Tools only). Ported from NegPy's WGSL:
`normalization.wgsl`, `exposure.wgsl`, `output_encode.wgsl`, `metrics.wgsl`.
