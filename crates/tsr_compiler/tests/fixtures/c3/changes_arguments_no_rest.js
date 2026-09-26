function f() {
  return arguments[0];
}
f("something");
function g(...args) {
  return args[0];
}
g("something");
