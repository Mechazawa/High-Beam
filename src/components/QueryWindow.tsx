import React, {KeyboardEventHandler, useCallback, useEffect, useMemo, useState} from "react";

import './QueryWindow.scss'
import QueryResult from "../plugins/interfaces/QueryResult";
import QueryResultRow from "./QueryResultRow";

function useQuery(): [(value: string) => void, Omit<QueryResult, 'call'>[]] {
  const query = (value: string) => window.ipcRenderer.send("query", value);
  const [results, setResults] = useState<Omit<QueryResult, 'call'>[]>([]);

  const handleResults = useCallback((_: never, data: Omit<QueryResult, 'call'>[]) => {
    setResults(data);
  }, [setResults]);

  useEffect(() => {
    window.ipcRenderer.on("result", handleResults);

    return () => window.ipcRenderer.off("result", handleResults);
  });

  return [query, results];
}

function useArrowIndex(max: number): [number, (index: number) => void] {
  const [index, setIndex] = useState(-1);
  const updateIndex = useCallback((value: number) => {
    setIndex(Math.max(-1, Math.min(value, max - 1)))

    console.log(value);
  }, [max]);

  const handleArrowKey = useCallback((event: KeyboardEvent) => {
    switch (event.key) {
      case 'ArrowUp':
        updateIndex(index - 1)
        break;
      case 'ArrowDown':
        updateIndex(index + 1);
        break
    }
  }, [max, index]);

  useEffect(() => {
    document.addEventListener('keydown', handleArrowKey);

    return () => document.removeEventListener('keydown', handleArrowKey);
  }, [handleArrowKey]);

  return [index, updateIndex];
}

export default function QueryWindow() {
  const [query, results] = useQuery();
  const [arrowIndex, setArrowIndex] = useArrowIndex(results.length);

  function handleQueryChange(event: React.ChangeEvent<HTMLInputElement>) {
    query(event.target.value);
  }

  // todo neater way
  const callIndex = (index: number) => {
    const token = results[index]?.token;

    if (!token) return;

    // todo meta
    window.ipcRenderer.send('select', token, false);
  };

  const handleInputKeyDown = (event: React.KeyboardEvent) => {
    if (event.key !== 'Enter') return;

    callIndex(Math.max(0, arrowIndex));
  }

  return (
    <>
      <input onKeyDown={handleInputKeyDown} placeholder="Query..." className="query" onChange={handleQueryChange}
             autoFocus/>
      {results.map((result, index) => (
        <span
          onMouseEnter={() => setArrowIndex(index)}
          onClick={() => callIndex(index)}
        >
          <QueryResultRow
            highlight={index === arrowIndex}
            extended={false}
            index={index}
            key={index}
            {...result}
          />
        </span>
      ))}
    </>
  );
}