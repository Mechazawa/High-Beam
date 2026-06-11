import { describe, expect, it } from 'vitest';
import { query, TutorialView } from './plugin.js';

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

describe('tutorial plugin', () => {
    it('answers the tutorial / help / welcome keywords (and their prefixes)', async () => {
        for (const q of ['tutorial', 'help', 'welcome', 'tut', 'hel']) {
            const results = await collect(query(q));
            expect(results, q).toHaveLength(1);
            expect(results[0]).toMatchObject({
                key: 'tutorial-open',
                action: { kind: 'showView' },
            });
        }
    });

    it('stays silent for unrelated or too-short queries', async () => {
        for (const q of ['', 'a', 'to', 'calc', 'smoke']) {
            expect(await collect(query(q)), q).toHaveLength(0);
        }
    });

    it('renders the getting-started view with action buttons', () => {
        const tree = TutorialView.render.call(TutorialView.setup());

        expect(tree.title).toBe('Welcome to High Beam');
        expect(tree.body[0]).toMatchObject({ kind: 'heading', text: 'Welcome to High Beam' });

        const actions = tree.body.find((block) => block.kind === 'stack');
        const buttons = actions.children.filter((child) => child.kind === 'button');
        expect(buttons).toHaveLength(2);
        expect(buttons.map((b) => b.onClick.kind)).toEqual(['openUrl', 'closeView']);
    });
});
