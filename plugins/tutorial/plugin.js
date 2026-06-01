// tutorial — first-launch getting-started guide.
//
// Shown automatically the first time High Beam runs (the daemon fires the
// preset `tutorial` query and auto-opens this view). Reachable any time by
// typing `tutorial`, `help`, or `welcome`.

import { showView, openUrl, closeView } from 'highbeam:actions';
import { Heading, Text, Divider, Stack, Button } from 'highbeam:view';

const REPO_URL = 'https://github.com/Mechazawa/high-beam';

export const TutorialView = {
    setup: () => ({}),

    render() {
        return {
            title: 'Welcome to High Beam',
            body: [
                Heading({ text: 'Welcome to High Beam' }),
                Text({ text: 'A keyboard launcher. Here is everything you need to get going.' }),
                Divider(),
                Text({ text: 'Open it', size: 'lg' }),
                Text({ text: 'macOS: Shift+Space. Linux: bind highbeam --open to a hotkey.', tone: 'muted' }),
                Text({ text: 'Search', size: 'lg' }),
                Text({ text: 'Type to query, Up/Down to highlight, Enter to run, Esc to dismiss.', tone: 'muted' }),
                Text({ text: 'Built-in verbs', size: 'lg' }),
                Text({ text: 'Type settings, install <manifest-url>, reload, or update.', tone: 'muted' }),
                Text({ text: 'Plugins', size: 'lg' }),
                Text({ text: 'Features are single-file JS plugins. Add more with install <manifest-url>.', tone: 'muted' }),
                Divider(),
                Stack({
                    direction: 'h',
                    gap: 'sm',
                    children: [
                        Button({ label: 'Read the docs', tone: 'primary', onClick: openUrl(REPO_URL) }),
                        Button({ label: 'Got it', onClick: closeView }),
                    ],
                }),
            ],
        };
    },
};

const KEYWORDS = ['tutorial', 'help', 'welcome'];

export async function* query(input) {
    const q = input.trim().toLowerCase();
    if (q.length < 3) return;
    if (!KEYWORDS.some((keyword) => keyword.startsWith(q))) return;
    yield {
        key: 'tutorial-open',
        title: 'High Beam tutorial',
        subtitle: 'Getting started with the launcher',
        weight: 100,
        action: showView(TutorialView),
    };
}
