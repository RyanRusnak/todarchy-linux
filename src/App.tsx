// App shell — forwards to the ported design-mock UI in ./ui/app.jsx.
// Keeping a thin wrapper here makes it easy to swap the port out later if
// we decide to convert the big JSX tree to TypeScript.
import App from './ui/app.jsx';

export default App;
