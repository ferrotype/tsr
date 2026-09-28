/** @param {{ name: string }} props */
function Hello(props) { return <div>{props.name}</div>; }
export const ok = <Hello name="x" />;
export const bad = <Hello name={1} />;
export const missing = <Hello />;
