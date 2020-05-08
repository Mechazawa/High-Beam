import AbstractKeywordPlugin from "./AbstractKeywordPlugin";
import iconPath from "../assets/d20.svg";
import spells from "../assets/5eSpells.json";
import { exec } from "child_process";
import { readFileSync } from "fs";
import Fuse from "fuse.js";
import { highlightFuseMatches } from "../utils/data";
import { capitalize } from "../utils/string";

const icon = `data:image/svg+xml;base64,${readFileSync(__dirname + '/' + iconPath).toString('base64')}`;

export class DnDPlugin extends AbstractKeywordPlugin {
  name = "DnD 5e";

  keywords = [
    /^\s*(\w+) (.*)/i,
    /^5e\s*(\w+)/i,
    /^5e\s*(\w+) (.*)/i,
  ];

  keyword ([, type, query = '']) {
    type = type.toLowerCase();

    switch (type) {
      case 'spell':
      case 'spells':
        return this.getSpells(query);
      default:
        return [
          'spells',
        ].filter(name => name.startsWith(type)).map(name => ({
          title: `5e ${name} `,
          weight: (type.length / name.length) * 100,
          key: name,
          pluginName: this.name,
          icon,
        }));
    }
  }

  getSpells (query) {
    const matches = new Fuse(spells, {
      includeScore: true,
      includeMatches: true,
      keys: ['name', 'level', 'school', 'classes'],

    }).search(query).filter(({ score }) => score <= .7);

    matches.sort((a, b) => (1e9 * a.score) - (1e9 * b.score));
    matches.splice(10, matches.length);

    return matches.map(match => {
        const {
          name, level, school, classes,
          href, materials, description,
          range, components, duration,
          'higher levels': higherLevels,
          'casting time': castingTime,
        } = { ...match.item, ...highlightFuseMatches(match.matches) };

        const shortDescription = `
        ${level === 'cantrip' ? '' : 'Level'} ${level.replace(/\w/, x => x.toUpperCase())}, ${classes}, ${school.replace(/\w/, x => x.toUpperCase())}, ${range}, 
        ${duration}, ${castingTime}, ${components}
      `.replace(/,\s+/g, ', ').trim();

        let descriptionExtended = `<i>${level === 'cantrip' ? 'Cantrip' : `${level}th level`} ${school}</i><br>`;

        const tableData = {
          'casting time': castingTime,
          range, components, materials,
          duration, description,
          'higher levels': higherLevels,
        };

        for (const [key, value] of Object.entries(tableData)) {
          if (!value) continue;

          descriptionExtended += `<strong>${capitalize(key)}:</strong> ${value}<br/>`;
        }

        return {
          icon,
          pluginName: this.name,
          key: href,
          title: name,
          description: shortDescription,
          descriptionExtended,
          html: true,
          weight: 100 - (match.score * 100),
        };
      },
    );
  }

  select (key) {
    if (key.startsWith('http')) {
      exec("open " + key);
    }
  }
}