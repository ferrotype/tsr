declare namespace React {
  function createElement(...args: any[]): JSX.Element;
  const Fragment: (props: { children?: any }) => JSX.Element;
  class Component<P> { constructor(props: P); props: P; render(): JSX.Element | null; }
  namespace JSX {
    interface Element { readonly el: true }
    interface ElementClass { render(): Element | null }
    interface ElementAttributesProperty { props: {} }
    interface ElementChildrenAttribute { children: {} }
    interface IntrinsicAttributes { key?: string | number }
    interface IntrinsicClassAttributes<T> { ref?: (instance: T) => void }
    type LibraryManagedAttributes<C, P> = C extends { defaultProps: infer D } ? Omit<P, keyof D> & Partial<D> : P;
    interface IntrinsicElements { div: { id?: string; children?: any }; "svg:rect": { width: number } }
  }
}
function Fn(props: { label: string; children: string }) { return <div>{props.label}</div>; }
class Cls extends React.Component<{ size: number; mode: "a" | "b" }> {
  static defaultProps = { mode: "a" as const };
  render() { return <div />; }
}
namespace ns { export function Comp(props: { x: boolean }) { return <div />; } }
export const e1 = <div id="i">text</div>;
export const e2 = <Fn label="l">child</Fn>;
export const e3 = <Fn label="l"><div /></Fn>;
export const e4 = <Cls size={1} />;
export const e5 = <Cls size="1" ref={(c) => c.props.size} />;
export const e6 = <ns.Comp x />;
export const e7 = <svg:rect width="1" />;
export const e8 = <><div /><Fn label="l">x</Fn></>;
export const e9 = <div unknownProp />;
export const e10 = <Fn label="l">a{"b"}</Fn>;
