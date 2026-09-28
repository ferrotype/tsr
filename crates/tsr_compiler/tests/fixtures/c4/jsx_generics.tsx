declare namespace React {
  function createElement(...args: any[]): JSX.Element;
  namespace JSX {
    interface Element { readonly el: true }
    interface IntrinsicElements { div: { children?: any } }
    interface ElementChildrenAttribute { children: {} }
  }
}
declare function Select<T>(props: { items: T[]; onPick: (item: T) => void; render?: (item: T) => string }): React.JSX.Element;
declare function Over(props: { kind: "a"; a: number }): React.JSX.Element;
declare function Over(props: { kind: "b"; b: string }): React.JSX.Element;
declare function Kids<T>(props: { value: T; children: (value: T) => React.JSX.Element }): React.JSX.Element;
export const g1 = <Select items={[1, 2]} onPick={(n) => n.toFixed()} />;
export const g2 = <Select<string> items={["a"]} onPick={(s) => s.toUpperCase()} />;
export const g3 = <Select<string> items={[1]} onPick={(s) => s} />;
export const g4 = <Over kind="a" a={1} />;
export const g5 = <Over kind="b" b={1} />;
export const g6 = <Kids value="v">{(v) => <div>{v.length}</div>}</Kids>;
export const g7 = <Kids value={1}>{(v) => <div>{v.length}</div>}</Kids>;
