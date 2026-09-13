/// <reference types="bun-types" />
import { describe, expect, test } from 'bun:test';
import { buildMemoryTree, type MemoryTreeNode } from './memory_tree';

interface Entry {
  slug: string;
}

function expectFolder<T>(node: MemoryTreeNode<T>) {
  if (node.type !== 'folder') throw new Error(`expected folder, got ${node.type}`);
  return node;
}

describe('buildMemoryTree', () => {
  test('empty descriptor list returns an empty tree', () => {
    expect(buildMemoryTree<Entry>([])).toEqual([]);
  });

  test('slugs with no slash render as flat top-level leaves', () => {
    const entries: Entry[] = [{ slug: 'alpha' }, { slug: 'beta' }];
    const tree = buildMemoryTree(entries);
    expect(tree).toEqual([
      { type: 'leaf', slug: 'alpha', name: 'alpha', entry: entries[0] },
      { type: 'leaf', slug: 'beta', name: 'beta', entry: entries[1] }
    ]);
  });

  test('one level of nesting groups siblings under a shared folder', () => {
    const entries: Entry[] = [{ slug: 'security/timing' }, { slug: 'security/injection' }];
    const tree = buildMemoryTree(entries);
    expect(tree).toHaveLength(1);
    const folder = expectFolder(tree[0]);
    expect(folder.name).toBe('security');
    expect(folder.path).toBe('security');
    expect(folder.children).toEqual([
      { type: 'leaf', slug: 'security/timing', name: 'timing', entry: entries[0] },
      { type: 'leaf', slug: 'security/injection', name: 'injection', entry: entries[1] }
    ]);
  });

  test('multiple levels of nesting build a nested folder chain', () => {
    const entries: Entry[] = [{ slug: 'a/b/c' }];
    const tree = buildMemoryTree(entries);
    const a = expectFolder(tree[0]);
    expect(a.path).toBe('a');
    expect(a.children).toHaveLength(1);
    const b = expectFolder(a.children[0]);
    expect(b.path).toBe('a/b');
    expect(b.children).toEqual([{ type: 'leaf', slug: 'a/b/c', name: 'c', entry: entries[0] }]);
  });

  test('a slug that is both a leaf and a folder prefix renders as sibling nodes', () => {
    const entries: Entry[] = [{ slug: 'foo' }, { slug: 'foo/bar' }];
    const tree = buildMemoryTree(entries);
    expect(tree).toHaveLength(2);
    expect(tree[0]).toEqual({ type: 'leaf', slug: 'foo', name: 'foo', entry: entries[0] });
    const folder = expectFolder(tree[1]);
    expect(folder.path).toBe('foo');
    expect(folder.children).toEqual([
      { type: 'leaf', slug: 'foo/bar', name: 'bar', entry: entries[1] }
    ]);
  });

  test('two entries sharing a multi-segment prefix reuse the same folder chain', () => {
    const entries: Entry[] = [{ slug: 'a/b/c' }, { slug: 'a/b/d' }, { slug: 'a/e' }];
    const tree = buildMemoryTree(entries);
    expect(tree).toHaveLength(1);
    const a = expectFolder(tree[0]);
    expect(a.children).toHaveLength(2);
    const b = expectFolder(a.children[0]);
    expect(b.path).toBe('a/b');
    expect(b.children.map((n) => n.type === 'leaf' && n.slug)).toEqual(['a/b/c', 'a/b/d']);
    expect(a.children[1]).toEqual({ type: 'leaf', slug: 'a/e', name: 'e', entry: entries[2] });
  });

  test('a slug repeating its folder name is the folder master, not a sibling leaf', () => {
    const entries: Entry[] = [{ slug: 'coding/coding' }, { slug: 'coding/dry' }];
    const tree = buildMemoryTree(entries);
    expect(tree).toHaveLength(1);
    const folder = expectFolder(tree[0]);
    expect(folder.master).toEqual(entries[0]);
    expect(folder.children).toEqual([
      { type: 'leaf', slug: 'coding/dry', name: 'dry', entry: entries[1] }
    ]);
  });

  test('a lone master entry still renders its folder, with empty children', () => {
    const entries: Entry[] = [{ slug: 'comments/comments' }];
    const tree = buildMemoryTree(entries);
    expect(tree).toHaveLength(1);
    const folder = expectFolder(tree[0]);
    expect(folder.path).toBe('comments');
    expect(folder.master).toEqual(entries[0]);
    expect(folder.children).toEqual([]);
  });

  test('a master at a nested prefix attaches to its immediate parent folder', () => {
    const entries: Entry[] = [{ slug: 'a/b/b' }];
    const tree = buildMemoryTree(entries);
    const a = expectFolder(tree[0]);
    expect(a.master).toBeUndefined();
    const b = expectFolder(a.children[0]);
    expect(b.path).toBe('a/b');
    expect(b.master).toEqual(entries[0]);
    expect(b.children).toEqual([]);
  });

  test('a top-level leaf and a same-named folder master coexist without merging', () => {
    const entries: Entry[] = [{ slug: 'foo' }, { slug: 'foo/foo' }];
    const tree = buildMemoryTree(entries);
    expect(tree).toHaveLength(2);
    expect(tree[0]).toEqual({ type: 'leaf', slug: 'foo', name: 'foo', entry: entries[0] });
    const folder = expectFolder(tree[1]);
    expect(folder.path).toBe('foo');
    expect(folder.master).toEqual(entries[1]);
    expect(folder.children).toEqual([]);
  });
});
