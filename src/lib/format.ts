export function bytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return new Intl.NumberFormat('pt-BR', {
    maximumFractionDigits: index > 0 ? 1 : 0,
  }).format(value / (1024 ** index)) + ' ' + units[index];
}

export function number(value: number): string {
  return new Intl.NumberFormat('pt-BR').format(value);
}

export function duration(ms: number): string {
  return ms >= 1000 ? (ms / 1000).toFixed(1).replace('.', ',') + ' s' : ms + ' ms';
}

export function truncatePath(value: string, count = 80): string {
  if (value.length <= count) return value;
  return '…' + value.slice(-count + 1);
}
