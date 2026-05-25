// view-demo — exercises the highbeam:view runtime end-to-end.
//
// Type "view" (or "view demo") in the launcher and pick the row; the
// window switches to view mode and paints a Heading, several Text
// variants (sizes + tones), a Spinner, and a ProgressBar that fills
// over a few seconds via mounted() + reactive state.

import { showView } from 'highbeam:actions';
import { Heading, Text, Spinner, ProgressBar } from 'highbeam:view';

const DemoView = {
    setup: () => ({
        progress: 0,
        completed: 0,
        total: 10,
        done: false,
    }),

    async mounted({ signal }) {
        // Tick progress in 10 steps with ~200ms between each so the
        // bar visibly fills. Bail early if the user dismissed the
        // view mid-animation.
        for (let i = 1; i <= this.total; i++) {
            if (signal.aborted) return;
            await new Promise((r) => setTimeout(r, 200));
            if (signal.aborted) return;
            this.completed = i;
            this.progress = i / this.total;
        }
        this.done = true;
    },

    render() {
        return {
            title: 'View Demo',
            body: [
                Heading({ text: 'Hello from a plugin view' }),
                Text({
                    text: 'A Vue-style reactive screen, pushed onto the launcher stack by an action.',
                }),
                Text({ text: 'Tone variants:', size: 'sm', tone: 'muted' }),
                Text({ text: 'success — looks good', tone: 'success' }),
                Text({ text: 'warning — heads up', tone: 'warning' }),
                Text({ text: 'error — something broke', tone: 'error' }),
                Spinner({
                    label: this.done ? 'All done.' : 'Working…',
                }),
                ProgressBar({
                    value: this.progress,
                    label: `Step ${this.completed} / ${this.total}`,
                }),
                this.done
                    ? Text({ text: 'Press Esc to close.', size: 'sm', tone: 'muted' })
                    : Text({ text: '', size: 'sm' }),
            ],
        };
    },
};

export async function* query(input) {
    const q = input.toLowerCase().trim();
    if (q.length === 0) return;
    if (!'view demo'.startsWith(q) && !q.startsWith('view')) return;
    yield {
        key: 'view-demo-open',
        title: 'View demo',
        subtitle: 'Open the sample plugin view',
        weight: 50,
        action: showView(DemoView),
    };
}
