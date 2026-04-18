import { invoke } from '@tauri-apps/api/core';
import type { DiagReport } from '../types';

export const runDiagnose = () => invoke<DiagReport>('run_diagnose');
