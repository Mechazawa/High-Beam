// Echo plugin — minimal smoke-test fixture. Yields one row that copies the
// current input to the clipboard.

import { copy } from "highbeam:actions";

export async function* query(input, _signal) {
    if (!input) return;
    yield {
        key: "echo",
        title: `echo: ${input}`,
        subtitle: "press Enter to copy to clipboard",
        action: copy(input),
    };
}
