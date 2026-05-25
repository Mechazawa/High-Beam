import { copy } from "highbeam:actions";

// Hand-rolled shunting-yard evaluator — keeps the plugin npm-free.

const CONSTANTS = {
    pi: Math.PI,
    e: Math.E,
};

const FUNCTIONS = {
    sqrt: { arity: 1, fn: (x) => Math.sqrt(x) },
    abs: { arity: 1, fn: (x) => Math.abs(x) },
    floor: { arity: 1, fn: (x) => Math.floor(x) },
    ceil: { arity: 1, fn: (x) => Math.ceil(x) },
    round: { arity: 1, fn: (x) => Math.round(x) },
    min: { arity: -1, fn: (...xs) => Math.min(...xs) },
    max: { arity: -1, fn: (...xs) => Math.max(...xs) },
    pow: { arity: 2, fn: (a, b) => a ** b },
};

// `**` is right-associative; everything else left-associative. Unary minus
// gets its own precedence above `**`.
const BINARY_OPS = {
    "+": { prec: 1, assoc: "left", fn: (a, b) => a + b },
    "-": { prec: 1, assoc: "left", fn: (a, b) => a - b },
    "*": { prec: 2, assoc: "left", fn: (a, b) => a * b },
    "/": { prec: 2, assoc: "left", fn: (a, b) => a / b },
    "%": { prec: 2, assoc: "left", fn: (a, b) => a % b },
    "**": { prec: 4, assoc: "right", fn: (a, b) => a ** b },
};

const UNARY_PREC = 5;

function tokenize(src) {
    const tokens = [];
    let i = 0;
    while (i < src.length) {
        const c = src[i];
        if (c === " " || c === "\t" || c === "\n" || c === "\r") {
            i += 1;
            continue;
        }
        if ((c >= "0" && c <= "9") || c === ".") {
            let j = i;
            while (j < src.length && ((src[j] >= "0" && src[j] <= "9") || src[j] === ".")) {
                j += 1;
            }
            const text = src.slice(i, j);
            const value = Number(text);
            if (!Number.isFinite(value)) throw new Error("bad number");
            tokens.push({ kind: "num", value });
            i = j;
            continue;
        }
        if ((c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || c === "_") {
            let j = i;
            while (
                j < src.length &&
                ((src[j] >= "a" && src[j] <= "z") ||
                    (src[j] >= "A" && src[j] <= "Z") ||
                    (src[j] >= "0" && src[j] <= "9") ||
                    src[j] === "_")
            ) {
                j += 1;
            }
            tokens.push({ kind: "ident", name: src.slice(i, j).toLowerCase() });
            i = j;
            continue;
        }
        if (c === "*" && src[i + 1] === "*") {
            tokens.push({ kind: "op", op: "**" });
            i += 2;
            continue;
        }
        if ("+-*/%".includes(c)) {
            tokens.push({ kind: "op", op: c });
            i += 1;
            continue;
        }
        if (c === "(" || c === ")" || c === ",") {
            tokens.push({ kind: c });
            i += 1;
            continue;
        }
        throw new Error(`unexpected character: ${c}`);
    }
    return tokens;
}

function applyOp(op, stack) {
    if (op.kind === "unary") {
        if (stack.length < 1) throw new Error("missing operand");
        const v = stack.pop();
        stack.push(-v);
        return;
    }
    if (op.kind === "fn") {
        const def = FUNCTIONS[op.name];
        if (def.arity !== -1 && op.argc !== def.arity) {
            throw new Error(`arity mismatch for ${op.name}`);
        }
        if (stack.length < op.argc) throw new Error("missing operand");
        const args = stack.splice(stack.length - op.argc, op.argc);
        stack.push(def.fn(...args));
        return;
    }
    if (stack.length < 2) throw new Error("missing operand");
    const b = stack.pop();
    const a = stack.pop();
    stack.push(BINARY_OPS[op.op].fn(a, b));
}

// Standard shunting-yard precedence/associativity rule, generalised to
// unary ops and function frames.
function shouldPopBefore(ops, incoming) {
    if (ops.length === 0) return false;
    const top = ops[ops.length - 1];
    if (top.kind === "lparen" || top.kind === "fn-open") return false;
    if (top.kind === "unary") return UNARY_PREC >= BINARY_OPS[incoming].prec;
    if (top.kind === "fn") return true;
    const topInfo = BINARY_OPS[top.op];
    const incInfo = BINARY_OPS[incoming];
    if (topInfo.prec > incInfo.prec) return true;
    return topInfo.prec === incInfo.prec && incInfo.assoc === "left";
}

function parseAndEvaluate(src) {
    const tokens = tokenize(src);
    if (tokens.length === 0) throw new Error("empty");
    const output = [];
    const ops = [];
    // Distinguishes unary minus from binary minus and allows `()` only after
    // an operator / open paren / function call.
    let expectOperand = true;
    // Parallel to `ops`: per function frame, count args; `,` bumps it.
    const fnArgs = [];

    for (let idx = 0; idx < tokens.length; idx += 1) {
        const tok = tokens[idx];
        if (tok.kind === "num") {
            if (!expectOperand) throw new Error("unexpected number");
            output.push(tok.value);
            expectOperand = false;
            continue;
        }
        if (tok.kind === "ident") {
            if (!expectOperand) throw new Error("unexpected identifier");
            const next = tokens[idx + 1];
            if (next && next.kind === "(") {
                if (!FUNCTIONS[tok.name]) throw new Error(`unknown function ${tok.name}`);
                ops.push({ kind: "fn", name: tok.name, argc: 1 });
                ops.push({ kind: "fn-open" });
                fnArgs.push(ops.length - 2);
                idx += 1;
                expectOperand = true;
                continue;
            }
            if (CONSTANTS[tok.name] !== undefined) {
                output.push(CONSTANTS[tok.name]);
                expectOperand = false;
                continue;
            }
            throw new Error(`unknown identifier ${tok.name}`);
        }
        if (tok.kind === "(") {
            if (!expectOperand) throw new Error("unexpected `(`");
            ops.push({ kind: "lparen" });
            expectOperand = true;
            continue;
        }
        if (tok.kind === ")") {
            while (ops.length && ops[ops.length - 1].kind !== "lparen" && ops[ops.length - 1].kind !== "fn-open") {
                applyOp(ops.pop(), output);
            }
            if (ops.length === 0) throw new Error("mismatched `)`");
            const opener = ops.pop();
            if (opener.kind === "fn-open") {
                const fnIdx = fnArgs.pop();
                const fnFrame = ops[fnIdx];
                // Empty parens (`fn()`) → argc 0. We start at 1 assuming an
                // operand; reset here if none arrived.
                if (expectOperand && fnFrame.argc === 1) fnFrame.argc = 0;
                ops.splice(fnIdx, 1);
                applyOp(fnFrame, output);
            }
            expectOperand = false;
            continue;
        }
        if (tok.kind === ",") {
            if (fnArgs.length === 0) throw new Error("unexpected `,`");
            while (ops.length && ops[ops.length - 1].kind !== "fn-open") {
                applyOp(ops.pop(), output);
            }
            const fnIdx = fnArgs[fnArgs.length - 1];
            ops[fnIdx].argc += 1;
            expectOperand = true;
            continue;
        }
        if (tok.kind === "op") {
            if (expectOperand) {
                if (tok.op === "-") {
                    ops.push({ kind: "unary" });
                    continue;
                }
                if (tok.op === "+") continue;
                throw new Error(`unexpected operator ${tok.op}`);
            }
            if (!BINARY_OPS[tok.op]) throw new Error(`unexpected operator ${tok.op}`);
            while (shouldPopBefore(ops, tok.op)) {
                applyOp(ops.pop(), output);
            }
            ops.push({ kind: "binary", op: tok.op });
            expectOperand = true;
            continue;
        }
    }
    if (expectOperand) throw new Error("trailing operator");
    while (ops.length) {
        const top = ops.pop();
        if (top.kind === "lparen" || top.kind === "fn-open") throw new Error("mismatched `(`");
        applyOp(top, output);
    }
    if (output.length !== 1) throw new Error("malformed expression");
    return output[0];
}

// Trim float fuzz (e.g. `0.1 + 0.2`) without flipping into exponential
// notation for small magnitudes.
function formatResult(value) {
    if (Number.isInteger(value)) return String(value);
    const rounded = Math.round(value * 1e12) / 1e12;
    return String(rounded);
}

export async function* query(input, _signal) {
    if (!input || !input.trim()) return;
    // `=` prefix surfaces feedback during long-expression typing even when
    // the partial parse fails; bare form stays silent.
    const trimmed = input.trimStart();
    const forced = trimmed.startsWith("=");
    const expression = forced ? trimmed.slice(1) : input;

    if (!expression.trim()) {
        if (!forced) return;
        yield {
            key: `calc:=`,
            title: "Syntax error",
            subtitle: "empty expression",
            weight: 100,
            pinned: true,
            action: copy(input),
        };
        return;
    }

    let value;
    let error;
    try {
        value = parseAndEvaluate(expression);
    } catch (err) {
        error = err;
    }

    if (error === undefined && Number.isFinite(value)) {
        const text = formatResult(value);
        const row = {
            key: `calc:${expression.trim()}`,
            title: text,
            weight: 100,
            pinned: true,
            action: copy(text),
        };
        // `=` mode also surfaces the source expression as a subtitle.
        if (forced) row.subtitle = expression.trim();
        yield row;
        return;
    }
    if (!forced) return;

    // Divide-by-zero / overflow produce non-finite numbers without throwing,
    // so we describe those separately from real parser exceptions.
    const reason = error ? errorReason(error) : nonFiniteReason(value);
    yield {
        key: `calc:=${expression.trim() || expression}`,
        title: "Syntax error",
        subtitle: reason,
        weight: 100,
        pinned: true,
        action: copy(input),
    };
}

function errorReason(err) {
    const msg = err?.message ? String(err.message) : "parse error";
    return msg.length > 80 ? `${msg.slice(0, 77)}...` : msg;
}

function nonFiniteReason(value) {
    if (Number.isNaN(value)) return "result is NaN";
    if (value === Infinity || value === -Infinity) return "divide by zero or overflow";
    return "result not finite";
}
