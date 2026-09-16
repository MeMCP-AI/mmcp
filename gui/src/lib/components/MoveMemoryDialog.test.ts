/// <reference types="bun-types" />
// MoveMemoryDialog is the GUI trigger for the cross-group move
// primitive (feature 454). Proves the dialog:
//   - lists every group except the memory's current one as a target,
//   - calls the `move_memory` Tauri command with the right args,
//     including the renumber flag,
//   - renders the back-reference report and the sync-push note on
//     success, and never silently drops them,
//   - calls `onMoved` with the store's result so the caller can
//     follow the memory to its new home.
import { afterEach, describe, expect, mock, test } from 'bun:test';
import { cleanup, render, fireEvent, waitFor } from '@testing-library/svelte';
import type { MoveMemoryResult } from '$lib/types';

const invokeMock = mock((_cmd: string, _args?: unknown): Promise<unknown> => Promise.resolve(undefined));
mock.module('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

import { groupsStore } from '$lib/stores/groups.svelte';
import MoveMemoryDialog from './MoveMemoryDialog.svelte';

afterEach(() => {
  cleanup();
  invokeMock.mockClear();
  groupsStore.groups = [];
});

function seedGroups() {
  groupsStore.groups = [
    { group_id: 'src', slug: 'source-group', display_name: null, memory_count_hint: 1, scope: 'project' },
    { group_id: 'dst', slug: 'target-group', display_name: 'Target Group', memory_count_hint: 1, scope: 'project' },
    { group_id: 'other', slug: 'other-group', display_name: null, memory_count_hint: 1, scope: 'global' }
  ];
}

describe('MoveMemoryDialog', () => {
  test('excludes the source group from the target list', () => {
    seedGroups();
    const { getByRole, queryByRole } = render(MoveMemoryDialog, {
      props: { groupId: 'src', slug: 'my-memory', onClose: () => {}, onMoved: () => {} }
    });

    expect(queryByRole('option', { name: 'source-group' })).toBeNull();
    getByRole('option', { name: 'Target Group' });
    getByRole('option', { name: 'other-group' });
  });

  test('moves, reports back references, and calls onMoved', async () => {
    seedGroups();
    const result: MoveMemoryResult = {
      id: '019d0000-0000-7000-8000-000000000000',
      slug: 'my-memory',
      source_group: 'src',
      target_group: 'dst',
      target_commit_id: 'a'.repeat(40),
      source_commit_id: 'b'.repeat(40),
      renumbered: [3, 7],
      back_references: [
        { group_slug: 'other-group', memory_slug: 'linker', memory_id: 'x', kind: 'cross_group_link' }
      ],
      sync_push_note: 'a sync push of this memory is refused by the server until issue #457 lands'
    };
    invokeMock.mockImplementation(() => Promise.resolve(result));

    const onMoved = mock((_r: MoveMemoryResult) => {});
    const { getByRole, getByText } = render(MoveMemoryDialog, {
      props: { groupId: 'src', slug: 'my-memory', onClose: () => {}, onMoved }
    });

    const select = getByRole('combobox') as HTMLSelectElement;
    await fireEvent.change(select, { target: { value: 'dst' } });
    const checkbox = getByRole('checkbox') as HTMLInputElement;
    await fireEvent.click(checkbox);

    await fireEvent.click(getByRole('button', { name: 'Move' }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenLastCalledWith('move_memory', {
        groupId: 'src',
        slug: 'my-memory',
        targetGroupId: 'dst',
        renumber: true
      });
    });

    expect(onMoved).toHaveBeenCalledWith(result);
    getByText(/Tracker number renumbered 3 to 7/);
    getByText(/other-group:linker/);
    getByText(result.sync_push_note);
  });
});
