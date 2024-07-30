import Plugin, {ResultCollection} from "./interfaces/Plugin";
import {evaluate} from "mathjs";
// import clipboardy from "clipboardy";

export default class CalculatorPlugin extends Plugin {
  name = 'calculator';

  query(query: string): ResultCollection {
    try {
      const result = evaluate(query.trim() || '0');

      return [{
        // icon: this.iconFetcher.icon,
        title: String(result),
        weight: 100,
        call: () => void 0,
        // call: () => clipboardy.writeSync(String(result)),
      }];
    } catch {
      return [];
    }
  }
}