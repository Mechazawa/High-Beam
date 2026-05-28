// `highbeam:view` — block factories for the dynamic-view system. Each
// factory returns a plain `{ kind, ...props }` object; the host renders
// the tree and bounces user events back through `on*` handlers.
//
// See `docs/views.md` for the full reactivity contract, lifecycle, and
// keyboard model.

import type {
    Block,
    ButtonBlock,
    DividerBlock,
    HeadingBlock,
    ImageBlock,
    InputBlock,
    ProgressBlock,
    RowBlock,
    SpinnerBlock,
    StackBlock,
    TextAreaBlock,
    TextBlock,
} from './types';

/** Stack-style layout container. Default direction is `'v'`. */
export function Stack(opts: Omit<StackBlock, 'kind'>): StackBlock;

/** Horizontal rule. Pure separator — no props. */
export function Divider(): DividerBlock;

/** Section heading. Larger and heavier than `Text`; one per view is typical. */
export function Heading(opts: Omit<HeadingBlock, 'kind'>): HeadingBlock;

/** Body text. `size` and `tone` map to theme tokens, not pixels / colours. */
export function Text(opts: Omit<TextBlock, 'kind'>): TextBlock;

/** Indeterminate progress indicator. Use `ProgressBar` when you know `N / total`. */
export function Spinner(opts?: Omit<SpinnerBlock, 'kind'>): SpinnerBlock;

/**
 * Determinate progress bar. Omit `value` for indeterminate ("working, no
 * fixed total"); set it in `[0, 1]` for `N / total` step progress.
 */
export function ProgressBar(opts?: Omit<ProgressBlock, 'kind'>): ProgressBlock;

/**
 * Clickable button. `onClick` accepts a closure (event → re-render) or a
 * bare `Action` (host runs it). Both wire through the same callback
 * path; the bare-Action form is shorthand for `() => action`.
 */
export function Button(opts: Omit<ButtonBlock, 'kind'>): ButtonBlock;

/** Single-line text input. `value` is the controlled value. `onChange(value)`. */
export function Input(opts: Omit<InputBlock, 'kind'>): InputBlock;

/** Multi-line text input. `rows` is a hint; the host clamps to fit. */
export function TextArea(opts: Omit<TextAreaBlock, 'kind'>): TextAreaBlock;

/** Image. `src` must be a `data:` URI in v1 (remote URLs are post-v1). */
export function Image(opts: Omit<ImageBlock, 'kind'>): ImageBlock;

/**
 * List row inside a view. Same shape as a query `Result`'s row, re-usable
 * for picker-style screens (`onClick: showView(...)` to drill in).
 */
export function Row(opts: Omit<RowBlock, 'kind'>): RowBlock;

export type { Block } from './types';
