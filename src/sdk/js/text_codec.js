// UTF-8-only TextEncoder / TextDecoder. Conversion happens host-side
// (__highbeam_utf8_*); these classes are the spec-shaped wrappers.
(() => {
    if (globalThis.TextEncoder !== undefined) return;

    class TextEncoder {
        constructor() {
            this.encoding = "utf-8";
        }

        encode(input = "") {
            return __highbeam_utf8_encode(String(input));
        }
    }

    class TextDecoder {
        constructor(label = "utf-8") {
            const enc = String(label).trim().toLowerCase();
            if (enc !== "utf-8" && enc !== "utf8" && enc !== "unicode-1-1-utf-8") {
                throw new RangeError(`TextDecoder: unsupported encoding ${label} (utf-8 only)`);
            }
            this.encoding = "utf-8";
        }

        decode(input) {
            if (input === undefined) return "";
            // Normalise the BufferSource shapes to the Uint8Array the host
            // hook expects; anything else falls through and the host throws.
            const bytes =
                input instanceof Uint8Array ? input
                : input instanceof ArrayBuffer ? new Uint8Array(input)
                : ArrayBuffer.isView(input) ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
                : input;
            return __highbeam_utf8_decode(bytes);
        }
    }

    globalThis.TextEncoder = TextEncoder;
    globalThis.TextDecoder = TextDecoder;
})();
