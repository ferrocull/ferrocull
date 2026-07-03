# Thumbnails come from embedded JPEGs, not decoded RAW

Modern cameras embed one or more JPEG previews inside every RAW file. Ferrocull extracts the largest embedded JPEG (`thumbnail::extract_largest_preview`) and resizes that with SIMD (`fast_image_resize`), rather than decoding the RAW. This is the trick that lets Photo Mechanic display 100 RAWs in ~12s where Lightroom takes ~190s.

The cost: the largest embedded JPEG is bounded by what the camera firmware writes (often 1600px on the long edge, sometimes full-size, rarely smaller). For 100% zoom on a 50MP RAW we can't go beyond that without falling back to RAW decode — a future concern, not a current one for culling.

If a future use case demands true RAW pixels (focus-peaking, full-res preview, develop-side processing), it lives on a separate path; the thumbnail/preview path stays embedded-JPEG.
