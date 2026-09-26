function loops(n: number) {
  let a: number;
  for (let i = 0; i < n; i++) {
    a = i;
  }
  return a;
}
function labels(n: number) {
  let b: number;
  outer: for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      if (j === 1) { b = j; break outer; }
    }
  }
  return b;
}
function finally_(n: number) {
  let c: number;
  try {
    c = n;
  } finally {
    c;
  }
  return c;
}
class Init {
  x: number;
  y: number = 1;
  constructor(flag: boolean) {
    if (flag) { this.x = 1; }
  }
}
