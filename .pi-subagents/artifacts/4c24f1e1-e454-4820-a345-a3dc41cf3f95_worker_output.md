Implemented round-1 Skyline Ace fixes and deterministic difficulty/targeting.

Algorithm: progress is `distance / 420m`, scaling capped health, cadence, projectile speed, lead, accuracy, density, and formation width. Enemy shots use bounded position/velocity intercept prediction with deterministic error; barrel rolls deflect incoming fire.

Changed files:
- `README.md`
- `package-lock.json`
- `src/game/{Game,Enemy,Laser,Arwing,Terrain,types,difficulty}.tsx/ts`

Validation:
- `npm install` passed
- `npm run build` passed
- Only Vite bundle-size warning remains