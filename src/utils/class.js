export const AbstractFieldSymbol = Symbol('AbstractField');

export class AbstractClassError extends Error {
  constructor () {
    super('Tried to load an abstract class');
  }
}

export function abstract (...args) {
  if (args.length === 3) {
    const descriptor = args[2];

    descriptor.value = AbstractFieldSymbol;

    return descriptor;
  } else if (args.length === 1) {
    const input = args[0];
    const constructor = Object.getPrototypeOf(input).constructor;

    Object.getPrototypeOf(input).constructor = function (...constructorArgs) {
      if (constructor === AbstractFieldSymbol || isAbstract(this)) {
        throw new AbstractClassError();
      }

      if (typeof constructor === 'function') {
        constructor.apply(this, constructorArgs);
      }
    }

    return input;
  }
}

// todo show what field was not implemented
export function isAbstract (self) {
  const proto = Object.getPrototypeOf(self);
  const constructor = Object.getPrototypeOf(self).constructor;

  return hasAnyAbstractFieldSymbols(proto) || hasAnyAbstractFieldSymbols(constructor);
}

function hasAnyAbstractFieldSymbols (obj) {
  const descriptors = Object.getOwnPropertyDescriptors(obj);

  return Object.entries(descriptors).some((desc) => desc.value === AbstractFieldSymbol);
}
