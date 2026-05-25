// view-demo — exercises the highbeam:view runtime end-to-end.
//
// Type "view" (or "view demo") in the launcher and pick the row; the
// window switches to view mode and paints a heading, sized + toned
// Text, a Spinner, a ProgressBar that fills via mounted(), a Button
// that mutates state on click (re-rendering on the spot), and an
// Input whose onChange feeds the text back into the same view.

import { showView, copy, closeView } from 'highbeam:actions';
import { Heading, Text, Spinner, ProgressBar, Button, Input } from 'highbeam:view';

const DemoView = {
    setup: () => ({
        progress: 0,
        completed: 0,
        total: 10,
        done: false,
        clicks: 0,
        message: '',
    }),

    async mounted({ signal }) {
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
                Text({ text: 'Reactive state — buttons and inputs feed back into render.' }),
                Text({ text: 'Tone variants:', size: 'sm', tone: 'muted' }),
                Text({ text: 'success — looks good', tone: 'success' }),
                Text({ text: 'warning — heads up', tone: 'warning' }),
                Text({ text: 'error — something broke', tone: 'error' }),
                Spinner({ label: this.done ? 'All done.' : 'Working…' }),
                ProgressBar({
                    value: this.progress,
                    label: `Step ${this.completed} / ${this.total}`,
                }),
                Text({ text: `Clicks: ${this.clicks}`, size: 'sm', tone: 'muted' }),
                Button({
                    label: 'Tap me',
                    tone: 'primary',
                    onClick: () => { this.clicks += 1; },
                }),
                Button({
                    label: 'Copy click count',
                    onClick: () => copy(`Clicks: ${this.clicks}`),
                }),
                Input({
                    id: 'message',
                    value: this.message,
                    placeholder: 'Type something — it echoes below',
                    onChange: (v) => { this.message = v; },
                }),
                this.message.length > 0
                    ? Text({ text: `You typed: ${this.message}` })
                    : Text({ text: '', size: 'sm' }),
                Button({
                    label: 'Close',
                    tone: 'danger',
                    onClick: () => closeView,
                }),
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
