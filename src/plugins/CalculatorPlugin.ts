import Plugin, {ResultCollection} from "./interfaces/Plugin";
import {evaluate} from "mathjs";
import ActionBuilder from "../ActionBuilder";

export default class CalculatorPlugin extends Plugin {
  name = 'calculator';

  query(query: string): ResultCollection {
    if (query.trim() === '') {
      return [];
    }

    try {
      const result = evaluate(query.trim() || '0');

      return [{
        // icon: this.iconFetcher.icon,
        title: String(result),
        weight: 100,
        call: ActionBuilder.copy(String(result)),
      }];
    } catch {
      return [];
    }
  }
}