// Builds a folder-hierarchy tree from a flat list of memory descriptors by splitting each `slug` on '/'.
// Pure data transform: no store access, no Svelte state.
// Unit-testable in isolation, reusable by any list/tree view over the same descriptor shape.
//
// A slug with no '/' is a top-level leaf.
// Each '/'-separated prefix becomes (or reuses) a folder node.
// Folders are keyed by their full prefix path, so two entries sharing a prefix share one folder node.
//
// Edge case: a slug can be both a leaf and a folder prefix (e.g. `foo` and `foo/bar` both exist).
// Chosen rule: `foo` renders as its own leaf node, sibling to the `foo/` folder node `foo/bar` introduces.
// Both sit at the same nesting depth.
// They are never merged into one node.
// A folder carries no frontmatter of its own to show.
// A leaf row and a folder row are different renderable shapes.
// Merging them would drop the leaf's row or invent frontmatter for the folder.

export interface MemoryTreeLeaf<T> {
  type: 'leaf';
  /** Full slug, unsplit, kept for keying and navigation. */
  slug: string;
  /** Last '/'-segment of the slug, the label shown for this row. */
  name: string;
  entry: T;
}

export interface MemoryTreeFolder<T> {
  type: 'folder';
  /** Single '/'-segment name, the label shown for this row. */
  name: string;
  /** Full '/'-joined prefix up to and including this folder, used as its stable identity. */
  path: string;
  children: MemoryTreeNode<T>[];
}

export type MemoryTreeNode<T> = MemoryTreeLeaf<T> | MemoryTreeFolder<T>;

/**
 * Builds the tree in input order.
 * Within a folder, children appear in the order their entries were first encountered.
 * Callers wanting alphabetical or kind-grouped order sort `entries` first; this function never reorders.
 */
export function buildMemoryTree<T extends { slug: string }>(
  entries: readonly T[]
): MemoryTreeNode<T>[] {
  const root: MemoryTreeNode<T>[] = [];
  const foldersByPath = new Map<string, MemoryTreeFolder<T>>();

  for (const entry of entries) {
    const segments = entry.slug.split('/');
    let siblings = root;
    let pathSoFar = '';
    for (let i = 0; i < segments.length - 1; i++) {
      pathSoFar = pathSoFar === '' ? segments[i] : `${pathSoFar}/${segments[i]}`;
      let folder = foldersByPath.get(pathSoFar);
      if (!folder) {
        folder = { type: 'folder', name: segments[i], path: pathSoFar, children: [] };
        foldersByPath.set(pathSoFar, folder);
        siblings.push(folder);
      }
      siblings = folder.children;
    }
    siblings.push({ type: 'leaf', slug: entry.slug, name: segments[segments.length - 1], entry });
  }

  return root;
}
