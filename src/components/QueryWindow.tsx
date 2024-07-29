import React, {ChangeEvent, FormEventHandler, useCallback, useEffect, useMemo, useState} from "react";

import './QueryWindow.scss'
import QueryResult from "../plugins/interfaces/QueryResult";
import QueryResultRow from "./QueryResultRow";

function useQuery(): [(value: string)=> void, Omit<QueryResult, 'call'>[]] {
  const query = (value: string) => window.ipcRenderer.send("query", value);
  const [results, setResults] = useState<Omit<QueryResult, 'call'>[]>([]);

  const handleResults = useCallback((event, data) => {
    console.debug("result", data);
    setResults(data);
  }, [setResults]);

  useEffect(() => {
    window.ipcRenderer.on("result", handleResults);

    return () => window.ipcRenderer.off("result", handleResults);
  });

  return [query, results];
}

export default function QueryWindow() {
  const [query, results] = useQuery();

  const resultRows = useMemo(() => {
    return results.map((result, index) => ({
      highlight: false,
      extended: false,
      index,
      ...result,
    })).map(QueryResultRow);
  }, [results]);

  function handleQueryChange(event: React.ChangeEvent<HTMLInputElement>) {
    query(event.target.value);
  }

  return (
    <>
      <input placeholder="Query..." className="query" onChange={handleQueryChange}/>
      { resultRows }
    </>
  );
}