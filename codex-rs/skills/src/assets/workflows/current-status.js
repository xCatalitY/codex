export const meta = {
  name: 'current-status',
  description: 'Report that bundled system workflows are installed and available.',
  whenToUse: 'Use as a read-only smoke check for the bundled workflow library.',
};

phase('status');
return {
  ok: true,
  source: 'system',
  workflow: workflowName,
  message: 'Bundled system workflows are installed and available.',
};
