declare function wrong(target: any, context: number): void;
function Box(props: { title: string }) { return <div id={1}>{props.title}</div>; }
export class K { @wrong m() {} }
export const x = <Box title={2} />;
