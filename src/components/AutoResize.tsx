import React, {ReactNode, useEffect, useRef} from "react";

interface props {
  children: ReactNode;
}

export default function AutoResize({children}: props) {
  const ref = useRef<Element>(null);

  const handleMutations = () => {
    const {width, height} = ref.current.getBoundingClientRect();

    // todo there is some flickering during resizing
    window.ipcRenderer.send("setBounds", {width, height});
  };

  useEffect(() => {
    if (ref.current) {
      const observer = new MutationObserver(handleMutations);

      observer.observe(ref.current, {
        attributes: true,
        childList: true,
        characterData: true,
      });

      requestAnimationFrame(handleMutations);

      return () => {
        observer.disconnect();
      };
    }
  }, []);

  return <div ref={ref}>{children}</div>;}