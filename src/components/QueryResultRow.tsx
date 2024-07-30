import React from 'react';
import classNames from "classnames";
import QueryResult from '../plugins/interfaces/QueryResult';
import './QueryResultRow.scss';

interface props extends Omit<QueryResult, 'call'> {
  highlight: boolean;
  index: number;
  extended: boolean;
}

export default function QueryResultRow({highlight, icon, title, description, extended, html, index}: props) {
  return (
    <div className={classNames('row', {highlight})}>
      {icon
        ? <img src={icon} className="icon cell" alt="icon"/>
        : <div className="icon cell"/>
      }
      <div className="cell">
        <strong className="cut-text">{title}</strong>
        {description && (
          <span className={classNames("description", {'cut-text': !extended})}>
            {html ? <div dangerouslySetInnerHTML={{__html: description}}/> : description}
          </span>
        )}
      </div>
    </div>
  )
}