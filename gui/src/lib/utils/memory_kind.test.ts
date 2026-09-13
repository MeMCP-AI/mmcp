/// <reference types="bun-types" />
import { describe, expect, test } from 'bun:test';
import { classifyMemoryKind, MEMORY_KIND_VALUES } from './memory_kind';

describe('incident kind', () => {
  test('is part of the generated kind vocabulary', () => {
    expect(MEMORY_KIND_VALUES).toContain('incident');
  });

  test('classifies into the generic memory bucket, like rule and log', () => {
    expect(classifyMemoryKind('incident')).toBe('memory');
  });
});
