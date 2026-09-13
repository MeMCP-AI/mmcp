/// <reference types="bun-types" />
// The mousedown guard is `preventBlurOnMouseDown` on MemoryTree.
// MemoryTree forwards it as `onMouseDown` on MemoryRow.
// It calls preventDefault on every row kind when set, never when unset.
// It never blocks the row's own click.
import { afterEach, describe, expect, mock, test } from 'bun:test';
import { cleanup, render } from '@testing-library/svelte';
import { flushSync } from 'svelte';
import MemoryTree, { type MemoryTreeEntry } from './MemoryTree.svelte';
import { buildMemoryTree } from '$lib/utils/memory_tree';

afterEach(() => {
  cleanup();
});

// Folder `a` masters at `a/a` and carries leaf `a/b`: a folder, chevron, and leaf all render.
function fixtureTree() {
  const entries: MemoryTreeEntry[] = [
    { slug: 'a/a', body: undefined },
    { slug: 'a/b', body: undefined }
  ];
  return buildMemoryTree(entries);
}

function dispatchMouseDown(el: Element): boolean {
  const event = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
  el.dispatchEvent(event);
  return event.defaultPrevented;
}

describe('MemoryTree mousedown guard', () => {
  test('guards every row without blocking its click', () => {
    const onSelect = mock((_slug: string) => {});
    const { getByRole } = render(MemoryTree, {
      props: {
        nodes: fixtureTree(),
        onSelect,
        forceExpand: false,
        preventBlurOnMouseDown: true
      }
    });

    const chevron = getByRole('button', { name: 'Expand folder' });
    expect(dispatchMouseDown(chevron)).toBe(true);
    chevron.click();
    flushSync();
    getByRole('button', { name: 'Collapse folder' });

    const folder = getByRole('button', { name: 'a: open a/a' });
    expect(dispatchMouseDown(folder)).toBe(true);
    folder.click();
    expect(onSelect).toHaveBeenLastCalledWith('a/a');

    const leaf = getByRole('button', { name: 'a/b a/b' });
    expect(dispatchMouseDown(leaf)).toBe(true);
    leaf.click();
    expect(onSelect).toHaveBeenLastCalledWith('a/b');
  });

  test('leaves every row unguarded when unset', () => {
    const onSelect = mock((_slug: string) => {});
    const { getByRole } = render(MemoryTree, {
      props: {
        nodes: fixtureTree(),
        onSelect,
        forceExpand: true,
        preventBlurOnMouseDown: false
      }
    });

    const chevron = getByRole('button', { name: 'Collapse folder' });
    expect(dispatchMouseDown(chevron)).toBe(false);

    const folder = getByRole('button', { name: 'a: open a/a' });
    expect(dispatchMouseDown(folder)).toBe(false);

    const leaf = getByRole('button', { name: 'a/b a/b' });
    expect(dispatchMouseDown(leaf)).toBe(false);
  });
});
